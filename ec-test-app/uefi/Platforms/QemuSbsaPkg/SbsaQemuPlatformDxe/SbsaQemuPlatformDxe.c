/** @file
*  FDT client protocol driver for qemu,mach-virt-ahci DT node
*
*  Copyright (c) 2019, Linaro Ltd. All rights reserved.
*
*  SPDX-License-Identifier: BSD-2-Clause-Patent
*
**/

#include <Library/BaseLib.h>
#include <Library/DebugLib.h>
#include <Library/NonDiscoverableDeviceRegistrationLib.h>
#include <Library/PcdLib.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/UefiDriverEntryPoint.h>
#include <Protocol/FdtClient.h>

#include <Library/ArmSmcLib.h>
#include <Library/BaseMemoryLib.h>
#include "SbsaQemuPlatform.h"


EFI_STATUS 
SetupSbsaQemuSharedMemory(VOID)
{
  EFI_STATUS status;

  // Chunk off this memory so the OS will not use it mark it reserved
  EFI_PHYSICAL_ADDRESS MemoryAddress = SBSAQEMU_RESERVED_MEMORY_BASE;
  UINT64 MemorySize = SBSAQEMU_RESERVED_MEMORY_SIZE;
  status = gBS->AllocatePages(
                  AllocateAddress,
                  EfiReservedMemoryType,
                  EFI_SIZE_TO_PAGES(MemorySize),
                  &MemoryAddress
                  );

  DEBUG ((DEBUG_ERROR, "Allocated address: 0x%llx size: 0x%llx status: 0x%x\n", 
            SBSAQEMU_RESERVED_MEMORY_BASE,
            SBSAQEMU_RESERVED_MEMORY_SIZE,
            status));
  
  ARM_SMC_ARGS  SmcArgs = {0};

  DEBUG ((DEBUG_INFO, "Send that we support FFA version 1.2 request\n"));
  ZeroMem(&SmcArgs, sizeof(SmcArgs));
  SmcArgs.Arg0 = FFA_VERSION_SMC;
  SmcArgs.Arg1 = 0x10002; // Indicate we support FFA Version 1.2
  ArmCallSmc (&SmcArgs);

  DEBUG ((DEBUG_ERROR, "    X0 = 0x%x\n", SmcArgs.Arg0));
  DEBUG ((DEBUG_ERROR, "    X1 = 0x%x\n", SmcArgs.Arg1));
  DEBUG ((DEBUG_ERROR, "    X2 = 0x%x\n", SmcArgs.Arg2));

  // Send FFA_RXTX_MAP to setup buffers which are required to sned FFA_MEMS_SHARE request
  DEBUG ((DEBUG_INFO, "Send FFA_RXTX_MAP request\n"));
  ZeroMem(&SmcArgs, sizeof(SmcArgs));
  SmcArgs.Arg0 = FFA_RXTX_MAP_SMC;
  SmcArgs.Arg1 = SBSAQEMU_TX_BUFFER_BASE; // TX buffer
  SmcArgs.Arg2 = SBSAQEMU_RX_BUFFER_BASE; // RX buffer
  SmcArgs.Arg3 = 0x1; // Number of 4K pages for each RX/TX buffer

  ArmCallSmc (&SmcArgs);
  DEBUG ((DEBUG_ERROR, "    X0 = 0x%x\n", SmcArgs.Arg0));
  DEBUG ((DEBUG_ERROR, "    X1 = 0x%x\n", SmcArgs.Arg1));
  DEBUG ((DEBUG_ERROR, "    X2 = 0x%x\n", SmcArgs.Arg2));

  // Populate the request
  ffa_memory_region_t *mem_req = (ffa_memory_region_t *)SBSAQEMU_TX_BUFFER_BASE; // TX_BUFFER
  mem_req->sender = 0; // OS VM is 0
  mem_req->attributes = 0x03; // No share device no cache device memory nonsecure 0b0101 0100
  mem_req->flags = 0;
  mem_req->handle = 0x0;
  mem_req->tag = SBSAQEMU_SHARED_MEM_TAG;
  mem_req->memory_access_desc_size = sizeof(ffa_memory_access_t);
  mem_req->receiver_count = 1;
  mem_req->receivers_offset = sizeof(ffa_memory_region_t);
  ffa_memory_access_t *memory_access = (ffa_memory_access_t *)((UINT64)mem_req + sizeof(ffa_memory_region_t));
  DEBUG ((DEBUG_ERROR, "memory_access = 0x%x\n", (UINT64)memory_access));

  memory_access->receiver_permissions.id = EC_SERVICE_VMID;
  memory_access->receiver_permissions.perm = 2; // 0b0010 no instruction access data RW
  memory_access->receiver_permissions.flags = 0;
  memory_access->composite_memory_region_offset = sizeof(ffa_memory_region_t) + sizeof(ffa_memory_access_t);
  memory_access->reserved_0 = 0;
  composite_memory_region_t *memory_region = (composite_memory_region_t *)((UINT64)memory_access + sizeof(ffa_memory_access_t));
  memory_region->total_page_count = 1;
  memory_region->address_range_count = 1;
  memory_region->reserved = 0;
  memory_region->regions[0].address = SBSAQEMU_SHARED_MEM_BASE;
  memory_region->regions[0].page_count = 1;

  // Send FFA request to share this memory
  DEBUG ((DEBUG_INFO, "Send FFA_MEM_SHARE request\n"));
  UINT32 len = sizeof(ffa_memory_region_t) + sizeof(ffa_memory_access_t) + sizeof(composite_memory_region_t);

  // Then register this test app to receive notifications from the Ffa test SP
  ZeroMem(&SmcArgs, sizeof(SmcArgs));
  SmcArgs.Arg0 = FFA_MEM_SHARE_SMC;
  SmcArgs.Arg1 = len; // Length of Transaction descriptor
  SmcArgs.Arg2 = len; // Length of Fragment
  SmcArgs.Arg3 = 0x0; // Address of buffer holding ffa_memory_access
  SmcArgs.Arg4 = 0x0; // Number of 4K pages

  ArmCallSmc (&SmcArgs);

  DEBUG ((DEBUG_ERROR, "    X0 = 0x%x\n", SmcArgs.Arg0));
  DEBUG ((DEBUG_ERROR, "    X1 = 0x%x\n", SmcArgs.Arg1));
  DEBUG ((DEBUG_ERROR, "    X2 = 0x%x\n", SmcArgs.Arg2));

  // If success the handle is in x2 so save that off
  mem_req->handle = SmcArgs.Arg2;

  // Copy the Memory descriptor over to the TX_BUFFER for SP which it will use to retrieve
  DEBUG ((DEBUG_INFO, "Send request to SP to fetch share memory region\n"));

  // Initalize some known value in shared memory buffer
  UINT64 *shared_mem = (UINT64 *)SBSAQEMU_SHARED_MEM_BASE;
  *shared_mem = 0xDEADBEEF;

  // Changed fields needed for the SP to retrieve this request
  memory_access->composite_memory_region_offset = 0x0;

  // Copy this into the TX buffer for EC svc so it can directly send
  void *sp_tx_buffer = (void *)EC_SVC_TX_BUFFER_BASE;
  CopyMem(sp_tx_buffer,mem_req,len);


  // Then register this test app to receive notifications from the Ffa test SP
  // <0x330c1273 0xfde54757 0x98195b65 0x39037502>
  ZeroMem(&SmcArgs, sizeof(SmcArgs));
  SmcArgs.Arg0 = FFA_MSG_SEND_DIRECT_REQ2_SMC;
  SmcArgs.Arg1 = EC_SERVICE_VMID; // Sender and receiver
  SmcArgs.Arg2 = EC_SVC_MANAGEMENT_GUID_LO; // uuid lo for FW Managment service
  SmcArgs.Arg3 = EC_SVC_MANAGEMENT_GUID_HI; // uuid hi for FW Management service
  SmcArgs.Arg4 = EC_CAP_MAP_SHARE;
  SmcArgs.Arg5 = (UINT64)sp_tx_buffer;
  SmcArgs.Arg6 = len;

  ArmCallSmc (&SmcArgs);

  DEBUG ((DEBUG_ERROR, "    X0 = 0x%x\n", SmcArgs.Arg0));
  DEBUG ((DEBUG_ERROR, "    X1 = 0x%x\n", SmcArgs.Arg1));
  DEBUG ((DEBUG_ERROR, "    X2 = 0x%x\n", SmcArgs.Arg2));


  // We need to unmap our RXTX buffers again so the OS can re-set them up again
  DEBUG ((DEBUG_INFO, "Send FFA_RXTX_UNMAP the OS will remap buffers again\n"));

  ZeroMem(&SmcArgs, sizeof(SmcArgs));
  SmcArgs.Arg0 = FFA_RXTX_UNMAP_SMC; // FFA_RXTX_UNMAP
  SmcArgs.Arg1 = 0x0; // VM ID 0x0 for OS

  ArmCallSmc (&SmcArgs);
  DEBUG ((DEBUG_ERROR, "    X0 = 0x%x\n", SmcArgs.Arg0));
  DEBUG ((DEBUG_ERROR, "    X1 = 0x%x\n", SmcArgs.Arg1));
  DEBUG ((DEBUG_ERROR, "    X2 = 0x%x\n", SmcArgs.Arg2));

  return EFI_SUCCESS;

}

EFI_STATUS
EFIAPI
InitializeSbsaQemuPlatformDxe (
  IN EFI_HANDLE        ImageHandle,
  IN EFI_SYSTEM_TABLE  *SystemTable
  )
{
  EFI_STATUS  Status;
  UINTN       Size;
  VOID        *Base;

  DEBUG ((DEBUG_INFO, "%a: InitializeSbsaQemuPlatformDxe called\n", __FUNCTION__));

  Base = (VOID *)(UINTN)PcdGet64 (PcdPlatformAhciBase);
  ASSERT (Base != NULL);
  Size = (UINTN)PcdGet32 (PcdPlatformAhciSize);
  ASSERT (Size != 0);

  DEBUG ((
    DEBUG_INFO,
    "%a: Got platform AHCI %llx %u\n",
    __FUNCTION__,
    Base,
    Size
    ));

  Status = RegisterNonDiscoverableMmioDevice (
             NonDiscoverableDeviceTypeAhci,
             NonDiscoverableDeviceDmaTypeCoherent,
             NULL,
             NULL,
             1,
             Base,
             Size
             );

  if (EFI_ERROR (Status)) {
    DEBUG ((
      DEBUG_ERROR,
      "%a: NonDiscoverable: Cannot install AHCI device @%p (Status == %r)\n",
      __FUNCTION__,
      Base,
      Status
      ));
    return Status;
  }

  Status = SetupSbsaQemuSharedMemory();

  return Status;
}

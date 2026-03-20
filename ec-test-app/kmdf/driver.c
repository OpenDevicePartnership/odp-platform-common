/*++
Module Name:
    driver.c

Abstract:
    This module contains the entry points and core routines for the WDF-based
    function driver. It includes initialization logic in DriverEntry, device
    creation in EvtDeviceAdd, and cleanup in DriverUnload. The driver also
    integrates WPP tracing for diagnostics and logging.

Environment:
    Kernel-mode only

--*/

#include "driver.h"
#include "trace.h"
#include "driver.tmh"

#ifdef ALLOC_PRAGMA
#pragma alloc_text (INIT, DriverEntry)
#pragma alloc_text (PAGE, EvtDeviceAdd)
#endif


NTSTATUS
DriverEntry(
    IN PDRIVER_OBJECT  DriverObject,
    IN PUNICODE_STRING RegistryPath
    )
/*++

Routine Description:
    DriverEntry initializes the driver and is the first routine called by the
    system after the driver is loaded. DriverEntry specifies the other entry
    points in the function driver, such as EvtDevice and DriverUnload.

Parameters Description:

    DriverObject - represents the instance of the function driver that is loaded
    into memory. DriverEntry must initialize members of DriverObject before it
    returns to the caller. DriverObject is allocated by the system before the
    driver is loaded, and it is released by the system after the system unloads
    the function driver from memory.

    RegistryPath - represents the driver specific path in the Registry.
    The function driver can use the path to store driver related data between
    reboots. The path does not store hardware instance specific data.

Return Value:

    STATUS_SUCCESS if successful,
    STATUS_UNSUCCESSFUL otherwise.

--*/
{
    WDF_DRIVER_CONFIG config;
    NTSTATUS status;

    // Initialize WPP tracing
    WPP_INIT_TRACING(DriverObject, RegistryPath);

    WDF_DRIVER_CONFIG_INIT(&config, EvtDeviceAdd );

    status = WdfDriverCreate(DriverObject,
                            RegistryPath,
                            WDF_NO_OBJECT_ATTRIBUTES,
                            &config,
                            WDF_NO_HANDLE);
    if (!NT_SUCCESS(status)) {
        Trace(TRACE_LEVEL_ERROR,TRACE_DRIVER,"Error: WdfDriverCreate failed 0x%x\n", status);
        return status;
    }

    return status;
}

NTSTATUS
EvtDeviceAdd(
    IN WDFDRIVER       Driver,
    IN PWDFDEVICE_INIT DeviceInit
    )
/*++
Routine Description:

    EvtDeviceAdd is called by the framework in response to AddDevice
    call from the PnP manager. We create and initialize a device object to
    represent a new instance of the device.

Arguments:

    Driver - Handle to a framework driver object created in DriverEntry

    DeviceInit - Pointer to a framework-allocated WDFDEVICE_INIT structure.

Return Value:

    NTSTATUS

--*/
{
    NTSTATUS status;

    UNREFERENCED_PARAMETER(Driver);
    PAGED_CODE();

    Trace(TRACE_LEVEL_INFORMATION,TRACE_DRIVER,"Enter  EvtDeviceAdd\n");
    status = ECTestDeviceCreate(DeviceInit);

    return status;
}

VOID
DriverUnload(
    _In_ PDRIVER_OBJECT DriverObject
    )
/*++
Routine Description:

    DriverUnload is called when driver is unloaded to cleanup WPP tracing

Arguments:

    Driver - Handle to a framework driver object created in DriverEntry

Return Value:

--*/
{
    // Clean up WPP tracing
    Trace(TRACE_LEVEL_INFORMATION,TRACE_DRIVER,"Enter  DriverUnload\n");
    WPP_CLEANUP(DriverObject);
}
/*++

Copyright (c) Microsoft Corporation. All rights reserved.

Module Name:

    ffa.h

Abstract:

    This module contains interface definitions and function prototypes exposed
    by the HAL's FF-A subcomponent.

Author:

    Kun Qin (kunqin)  10-Sep-2024

--*/

#pragma once

//
// -------------------------------------------------------- Macro Definitions
//

//
// FF-A notification macros
//

//
// Maximum number of FF-A notifications that can be enabled in a single
// FF-A call (constrained by the number of available SMC registers available) to
// the notification service. If caller desires to enable more notifications, it
// woudl need to break the enablement into multiple calls.
//

#define FFA_MAX_MAPPING_COUNT 10

//
// FF-A simple notification service GUID {B510B3A3-59F6-4054-BA7A-FF2EB1EAC765}
//

DEFINE_GUID(GUID_FFA_NOTIFY_SERVICE, 0xb510b3a3, 0x59f6, 0x4054, 0xba, 0x7a, 0xff, 0x2e, 0xb1, 0xea, 0xc7, 0x65);

//
// FF-A Status Reporting
//

#define FFA_ERROR 0x84000060
#define FFA_SUCCESS_AARCH32 0x84000061
#define FFA_SUCCESS_AARCH64 0xC4000061
#define FFA_INTERRUPT 0x84000062
#define FFA_OP_PAUSE 0xC4000097
#define FFA_OP_RESUME 0xC4000098
#define FFA_OP_ERROR 0xC400009A
#define FFA_RES_INFO_GET 0xC4000099
#define FFA_RES_AVAILABLE 0xC4000096
#define FFA_MSG_SEND_DIRECT_REQ2 0xC400008D
#define FFA_MSG_SEND_DIRECT_RESP2 0xC400008E

//
// FF-A Function IDs
//

#define FFA_VERSION 0x84000063
#define FFA_FEATURES 0x84000064
#define FFA_RX_ACQUIRE 0x84000084
#define FFA_RX_RELEASE 0x84000065
#define FFA_RXTX_MAP_AARCH32 0x84000066
#define FFA_RXTX_MAP_AARCH64 0xC4000066
#define FFA_RXTX_UNMAP 0x84000067
#define FFA_PARTITION_INFO_GET 0x84000068
#define FFA_PARTITION_INFO_GET_REGS 0xC400008B
#define FFA_ID_GET 0x84000069
#define FFA_SPM_ID_GET 0x84000085
#define FFA_CONSOLE_LOG_AARCH32 0x8400008A
#define FFA_CONSOLE_LOG_AARCH64 0xC400008A
#define FFA_MSG_WAIT 0x8400006B
#define FFA_YIELD 0x8400006C
#define FFA_RUN 0x8400006D
#define FFA_NORMAL_WORLD_RESUME 0x8400007C
#define FFA_MSG_SEND2 0x84000086
#define FFA_MSG_SEND_DIRECT_REQ_AARCH32 0x8400006F
#define FFA_MSG_SEND_DIRECT_REQ_AARCH64 0xC400006F
#define FFA_MSG_SEND_DIRECT_RESP_AARCH32 0x84000070
#define FFA_MSG_SEND_DIRECT_RESP_AARCH64 0xC4000070
#define FFA_MSG_SEND_DIRECT_REQ2 0xC400008D
#define FFA_MSG_SEND_DIRECT_RESP2 0xC400008E
#define FFA_NOTIFICATION_BITMAP_CREATE 0x8400007D
#define FFA_NOTIFICATION_BITMAP_DESTROY 0x8400007E
#define FFA_NOTIFICATION_BIND 0x8400007F
#define FFA_NOTIFICATION_UNBIND 0x84000080
#define FFA_NOTIFICATION_SET 0x84000081
#define FFA_NOTIFICATION_GET 0x84000082
#define FFA_NOTIFICATION_INFO_GET_AARCH32 0x84000083
#define FFA_NOTIFICATION_INFO_GET_AARCH64 0xC4000083
#define FFA_EL3_INTR_HANDLE 0x8400008C
#define FFA_SECONDARY_EP_REGISTER_AARCH32 0x84000087
#define FFA_SECONDARY_EP_REGISTER_AARCH64 0xC4000087

//
// Legacy FF-A Functionalities, below are commented out so that it will not get added later...
// #define FFA_MSG_SEND 0x8400006E
// #define FFA_MSG_POLL 0x8400006A
//

//
// FF-A Status Codes Type and Definitions
//

typedef LONG FFA_STATUS;

#define FFA_STATUS_SUCCESS 0
#define FFA_STATUS_ERROR_NOT_SUPPORTED -1
#define FFA_STATUS_ERROR_INVALID_PARAMETERS -2
#define FFA_STATUS_ERROR_NO_MEMORY -3
#define FFA_STATUS_ERROR_BUSY -4
#define FFA_STATUS_ERROR_INTERRUPTED -5
#define FFA_STATUS_ERROR_DENIED -6
#define FFA_STATUS_ERROR_RETRY -7
#define FFA_STATUS_ERROR_ABORTED -8
#define FFA_STATUS_ERROR_NO_DATA -9
#define FFA_STATUS_ERROR_NOT_READY -10

//
// FF-A Version Definitions
//

#define FFA_CALLER_VERSION_MAJOR 1
#define FFA_CALLER_VERSION_MINOR 2

typedef union _FFA_VERSION_NUMBER {
    struct {
        ULONG Minor : 16;
        ULONG Major : 15;
        ULONG Reserved : 1;
    };
    ULONG Raw;
} FFA_VERSION_NUMBER, *PFFA_VERSION_NUMBER;

//
// FF-A Features Definitions
//

#define FFA_FEATURE_NPI 0x00000001
#define FFA_FEATURE_SRI 0x00000002
#define FFA_FEATURE_MEI 0x00000003
#define FFA_FEATURE_NOTIFICATION 0x00000004
#define FFA_FEATURE_COMPLETION_MECH 0x00000005

//
// FF-A Notification Features
//

#define FFA_FEATURE_NOTIFICATION_PER_VCPU_MASK (1 << 0)

//
// FF-A Completion mechanism
//

#define FFA_FEATURE_COMPLETION_MECH_VALID_MASK (1 << 0)
#define FFA_FEATURE_COMPLETION_MECH_COOP_EN_MASK (1 << 1)

//
// FF-A Partition information descriptor definition
// The structure below corresponds to the FFA Partition Information Descriptor
// as defined in the FF-A specification. It was named to FF-A service info
// descriptor to match the main functionality of the structure.
//

typedef union _FFA_SERVICE_INFO_DESC {
    struct {
        ULONGLONG PartitionId : 16;
        ULONGLONG NumberOfExecutionContexts : 16;
        ULONGLONG PartitionProperties : 32;
    };
    ULONGLONG Raw;
} FFA_SERVICE_INFO_DESC, *PFFA_SERVICE_INFO_DESC;

#pragma pack(push, 1)
typedef struct _FFA_SERVICE_INFO {
    FFA_SERVICE_INFO_DESC ServiceInfoDesc;
    GUID ServiceUuid;
} FFA_SERVICE_INFO, *PFFA_SERVICE_INFO;
#pragma pack(pop)

//
// FF-A Notification Definitions
//

#define FFA_NOTIFICATIONS_FLAG_PER_VCPU (0x1 << 0)
#define FFA_NOTIFICATIONS_FLAG_BITMAP_SP (0x1 << 0)
#define FFA_NOTIFICATIONS_FLAG_BITMAP_VM (0x1 << 1)
#define FFA_NOTIFICATIONS_FLAG_BITMAP_SPM (0x1 << 2)
#define FFA_NOTIFICATIONS_FLAG_BITMAP_HYP (0x1 << 3)

//
// FF-A Parameter Structure
//

typedef struct _FFA_PARAMETERS {
    ULONGLONG Arg0;
    ULONGLONG Arg1;
    ULONGLONG Arg2;
    ULONGLONG Arg3;
    ULONGLONG Arg4;
    ULONGLONG Arg5;
    ULONGLONG Arg6;
    ULONGLONG Arg7;
    ULONGLONG Arg8;
    ULONGLONG Arg9;
    ULONGLONG Arg10;
    ULONGLONG Arg11;
    ULONGLONG Arg12;
    ULONGLONG Arg13;
    ULONGLONG Arg14;
    ULONGLONG Arg15;
    ULONGLONG Arg16;
    ULONGLONG Arg17;
} FFA_PARAMETERS, *PFFA_PARAMETERS;

//
// -------------------------------------------------------- Function Prototypes
//

NTSTATUS
FfaRawSmcCall (
    _In_ PFFA_PARAMETERS InputParameters,
    _Out_ PFFA_PARAMETERS OutputParameters
    );

NTSTATUS
FfaQueryVersion (
    _Out_ PFFA_VERSION_NUMBER Version
    );

NTSTATUS
FfaQueryFeature (
    _In_ ULONG FeatureId,
    _Out_ PFFA_PARAMETERS Parameters
    );

NTSTATUS
FfaQuerySriId (
    _Out_ PULONG SriId
    );

NTSTATUS
FfaQueryNotificationFeatures (
    _Out_ PULONGLONG NotificationFeatures
    );

NTSTATUS
FfaQueryPartitionInfo (
    _In_ PGUID ServiceId,
    _Inout_ PFFA_SERVICE_INFO ServiceInfo,
    _In_opt_ ULONG ServiceInfoBufferSize,
    _Out_ PULONG ServiceCount,
    _Out_ PULONG ServiceInfoSize
    );

NTSTATUS
FfaQueryAllServiceInfo (
    _Inout_ PFFA_SERVICE_INFO ServiceInfo,
    _In_opt_ ULONG ServiceInfoBufferSize,
    _Out_ PULONG ServiceCount,
    _Out_ PULONG ServiceInfoSize
    );

NTSTATUS
FfaQueryPartitionInfoRegs (
    _In_ PGUID ServiceId,
    _Out_ PFFA_SERVICE_INFO ServiceInfo
    );

NTSTATUS
FfaQueryId (
    _Out_ PUSHORT FfaId
    );

NTSTATUS
FfaEnableDisableNotification (
    _In_ USHORT PartitionId,
    _In_ PGUID ServiceId,
    _In_ USHORT MappingCount,
    _In_ PUSHORT BitmapIndices,
    _In_ PULONG NotifyIds,
    _In_ BOOLEAN Enable
    );

NTSTATUS
FfaRegisterRxTxBuffer (
    _In_ ULONGLONG RxBufferAddressVa,
    _In_ ULONGLONG TxBufferAddressVa,
    _In_ ULONGLONG RxBufferAddressPa,
    _In_ ULONGLONG TxBufferAddressPa,
    _In_ ULONGLONG BufferPageCount
    );

NTSTATUS
FfaUnregisterRxTxBuffer (
    VOID
    );

NTSTATUS
FfaReleaseRxBuffer (
    VOID
    );

NTSTATUS
FfaUnregisterRxTxBuffer (
    VOID
    );

NTSTATUS
FfaSendMsgSendDirectReq (
    _In_ USHORT PartitionId,
    _In_ PFFA_PARAMETERS InputParameters,
    _Out_ PFFA_PARAMETERS OutputParameters
    );

NTSTATUS
FfaSendMsgSendDirectReq2 (
    _In_ USHORT PartitionId,
    _In_ PGUID ServiceId,
    _In_opt_ PFFA_DIRECT_REQ2_ASYNC_PARAMETERS AsyncParameters,
    _In_ PFFA_PARAMETERS InputParameters,
    _Out_ PFFA_PARAMETERS OutputParameters
    );

NTSTATUS
FfaRun (
    _In_ PFFA_RUN_TARGET_INPUT_PARAMETERS RunInputParameters,
    _Out_ PFFA_RUN_TARGET_OUTPUT_PARAMETERS RunOutputParameters
    );

NTSTATUS
FfaNotificationBitMapCreate (
    ULONG VCpuCount
    );

NTSTATUS
FfaNotificationBitMapDestroy (
    VOID
    );

NTSTATUS
FfaNotificationBind (
    _In_ USHORT PartitionId,
    _In_ ULONGLONG Flags,
    _In_ ULONGLONG NotificationBitmap
    );

NTSTATUS
FfaNotificationUnbind (
    _In_ USHORT PartitionId,
    _In_ ULONGLONG NotificationBitmap
    );

NTSTATUS
FfaNotificationGet (
    _In_ USHORT VCpuId,
    _Inout_ PULONGLONG NotificationBitmap
    );

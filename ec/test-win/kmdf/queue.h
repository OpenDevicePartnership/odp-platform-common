/*++
Module Name:
    queue.h

Abstract:

    This is a C version of a very simple sample driver that illustrates
    how to use the driver framework and demonstrates best practices.
--*/

#include <acpiioct.h>

//
// This is the context that can be placed per queue
// and would contain per queue information.
//
typedef struct _WORKITEM_CONTEXT {
    WDFDEVICE Device;
    WDFQUEUE Queue;
    WDFREQUEST Request;
    ACPI_EVAL_INPUT_BUFFER_V1_EX *Buffer;
} WORKITEM_CONTEXT, *PWORKITEM_CONTEXT;

WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(WORKITEM_CONTEXT, WorkItemGetContext);

NTSTATUS
ECTestQueueInitialize(
    WDFDEVICE hDevice
    );

EVT_WDF_IO_QUEUE_CONTEXT_DESTROY_CALLBACK ECTestEvtIoQueueContextDestroy;

VOID
ECTestEvtIoDeviceControl(
    IN WDFQUEUE         Queue,
    IN WDFREQUEST       Request,
    IN size_t           OutputBufferLength,
    IN size_t           InputBufferLength,
    IN ULONG            IoControlCode
    );

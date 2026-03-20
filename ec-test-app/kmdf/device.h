/*++
Module Name:
    device.h

Abstract:
    This is a C version of a very simple sample driver that illustrates
    how to use the driver framework and demonstrates best practices.
--*/

#include "public.h"

#define EC_TEST_NOTIFICATIONS  // Enable notification support
//#define ENABLE_NOTIFICATION_SIMULATION // Enable notification simulation

//
// The device context performs the same job as
// a WDM device extension in the driver frameworks
//
typedef struct _DEVICE_CONTEXT
{
    WDFREQUEST PendingRequest; // Pending request for notification
#ifdef EC_TEST_NOTIFICATIONS
    WDFWAITLOCK  NotificationLock; // lock for notification
#endif
#if defined(EC_TEST_NOTIFICATIONS) && defined(ENABLE_NOTIFICATION_SIMULATION)
    WDFTIMER Timer; // Timer for notification simulation
#endif
} DEVICE_CONTEXT, *PDEVICE_CONTEXT;

//
// This macro will generate an inline function called DeviceContextGet
// which will be used to get a pointer to the device context memory
// in a type safe manner.
//
WDF_DECLARE_CONTEXT_TYPE_WITH_NAME(DEVICE_CONTEXT, DeviceContextGet)

//
// Function to initialize the device and its callbacks
//
NTSTATUS ECTestDeviceCreate(PWDFDEVICE_INIT DeviceInit );

#if defined(EC_TEST_NOTIFICATIONS) && defined(ENABLE_NOTIFICATION_SIMULATION)
// Timer routine to simulate receiving the Notification at the driver.
VOID TimerCallback(WDFTIMER Timer);
#endif

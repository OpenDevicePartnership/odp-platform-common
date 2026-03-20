/*
MIT License

Copyright (c) 2025 Open Device Partnership

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

#include <DriverSpecs.h>
_Analysis_mode_(_Analysis_code_type_user_code_)

#define INITGUID
#include <windows.h>
#include <strsafe.h>
#include <cfgmgr32.h>
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>
#include <SetupAPI.h>
#include <Devpkey.h>
#include <Acpiioct.h>
#include <devioctl.h>
#include <Objbase.h>
#include <memory>
#include "..\inc\ectest.h"

extern "C" {
    #include "..\inc\eclib.h"
}

#define EC_TEST_NOTIFICATIONS
//#define EC_TEST_SHARED_BUFFER

#define ACPI_OUTPUT_BUFFER_SIZE 1024
#define MAX_STRING_LEN 256
#define CMD_MIN_ARG_COUNT 3  // Always need ectest.exe -acpi <method>

// Global event handle
static HANDLE gExitEvent = NULL;

/*
 * Function: void DumpAcpi
 *
 * Description:
 * The DumpAcpi function evaluates an ACPI method on a specified device and prints the results.
 * It sends an IOCTL request to the device to execute the ACPI method and processes the returned data.
 *
 * Parameters:
 * methodName: Method of ACPI to evaluate and dump
 *
 * Return Value:
 * None.
 */
int DumpAcpi(ACPI_EVAL_INPUT_BUFFER_COMPLEX_V1_EX *acpiinput )
{

    BYTE buffer[ACPI_OUTPUT_BUFFER_SIZE];
    ACPI_EVAL_OUTPUT_BUFFER_V1 *AcpiOut = (ACPI_EVAL_OUTPUT_BUFFER_V1 *)buffer;
    size_t buffer_size = sizeof(buffer);

    int status = EvaluateAcpi((void *)acpiinput, sizeof(ACPI_EVAL_INPUT_BUFFER_COMPLEX_V1_EX) + acpiinput->Size, buffer, &buffer_size );

    if(status != ERROR_SUCCESS) {
        printf("EvaluateAcpi failed, status: 0x%x\n", status);
        return status;
    }

    // Print the raw output data returned from ACPI function
    printf("ACPI Method: \n");
    printf("  Signature: 0x%x\n", AcpiOut->Signature);
    printf("  Length: 0x%x\n", AcpiOut->Length);
    printf("  Count: 0x%x\n", AcpiOut->Count);

    // Dump out the contents of each Argument separately
    ACPI_METHOD_ARGUMENT_V1 *Argument = AcpiOut->Argument;

    for(ULONG i=0; i < AcpiOut->Count; i++) {
        printf("    Argument[%i]:\n", i);
        switch(Argument->Type) {
            case ACPI_METHOD_ARGUMENT_INTEGER:
                printf("    Integer Value: 0x%x\n", Argument->Argument);
                break;
            case ACPI_METHOD_ARGUMENT_STRING:
                printf("    String Value: %s\n", Argument->Data);
                break;
            case ACPI_METHOD_ARGUMENT_BUFFER:
            case ACPI_METHOD_ARGUMENT_PACKAGE:
            default:
                printf("    Buffer Data:\n");
                for(int j=0; j < Argument->DataLength; j++) {
                    printf(" 0x%x,", Argument->Data[j]);
                }
                break;

        }
        // Argument is variable length so update to point to next entry
        Argument = (ACPI_METHOD_ARGUMENT_V1 *)(Argument->Data + Argument->DataLength);
    }

    printf("\n\nACPI Raw Output:\n");
    for(ULONG i=0; i < AcpiOut->Length; i++) {
        printf(" 0x%x",((BYTE *)AcpiOut)[i]);
    }
    printf("\n\n");

    return ERROR_SUCCESS;
}

/*
 * Function: int CharToGUID
 *
 * Description:
 * This function converts an ASCII character to corresponding hex value or returns 0 if invalid
 *
 * Parameters:
 * out: Output BYTE array that contains GUID values
 * out_len: Length of output buffer must be at least 16 bytes
 * guid: Input pointer to char * of string representation of GUID
 * guid_len: Must be 39 bytes including terminating \0
 *
 * Return Value:
 * ERROR_SUCESS or failure code
 */
int CharToGUID(BYTE *out, size_t out_len, char *guid, size_t guid_len)
{
    // Make sure in and out buffers are lengths and format we expect
    if(out_len < 16 || guid_len != 39) {
        return ERROR_INVALID_PARAMETER;
    }
    
    // Convert char* to wide string
    wchar_t wideGuidStr[39]; // GUID string is 38 chars + null terminator
    size_t bytesReturned = 0;
    int status = mbstowcs_s(&bytesReturned, wideGuidStr, guid, guid_len);
    if( status != ERROR_SUCCESS) {
        return status;
    }

    IID uuid;
    HRESULT hr = IIDFromString(wideGuidStr, &uuid);

    if (SUCCEEDED(hr)) {
        // Copy data to guid buffer
        memcpy(out, &uuid, sizeof(IID));
    } else {
        return ERROR_INVALID_PARAMETER;
    }

    return ERROR_SUCCESS;
}

/*
 * Function: int ParseCmdline
 *
 * Description:
 * The ParseCmdline function parses the command line arguments and sets the ACPI method name if provided.
 * It checks the number of arguments and prints usage instructions if the required arguments are not provided.
 *
 * Parameters:
 * int argc: The number of command line arguments.
 * char **argv: The array of command line arguments.
 *
 * Return Value:
 * Returns ERROR_SUCCESS if the ACPI method name is successfully set, otherwise returns ERROR_INVALID_PARAMETER.
 */
int ParseCmdline(
    _In_ int argc,
    _In_ char ** argv
    )
{

    // Must always have at least 3 parameters
    if( argc < CMD_MIN_ARG_COUNT ) {
        printf("Usage:\n");
        printf("    ectest.exe                        --- Print this help\n");
        printf("    ectest.exe -acpi \\_SB.ECT0.NEVT  --- Evaluate given ACPI method with no arguments\n");
        printf("    ectest.exe -acpi \\_SB.ECT0.TDSM {07ff6382-e29a-47c9-ac87-e79dad71dd82} 1 3 0\n");
        printf("               GUID - {xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}\n");
        printf("            Integer - 0x123ABC 1234 -1234\n");
        printf("             String - \'TestString\'\n");

        return ERROR_INVALID_PARAMETER;
    } else if(argc > CMD_MIN_ARG_COUNT + 7) {
        // ACPI function cannot accept more than 7 arguments
        printf("Exceeded 7 ACPI arguments!\n");
        return ERROR_INVALID_PARAMETER;
    }

    // Create new buffer based on number of parameters and max string size
    size_t buffer_max = (argc-CMD_MIN_ARG_COUNT)*MAX_STRING_LEN + sizeof(ACPI_EVAL_INPUT_BUFFER_COMPLEX_V1_EX);
    std::unique_ptr<BYTE[]> buffer(new BYTE[buffer_max]); // Throws exception if it fails, auto frees

    auto* params = reinterpret_cast<ACPI_EVAL_INPUT_BUFFER_COMPLEX_V1_EX*>(buffer.get());
    params->Signature = ACPI_EVAL_INPUT_BUFFER_COMPLEX_SIGNATURE_EX;
    strncpy_s(params->MethodName, sizeof(params->MethodName), argv[2], strlen(argv[2]));
    params->ArgumentCount = argc - 3;
    params->Size = 0;


    printf("Signature: 0x%x\n", params->Signature);

    // Iterate through the argument creation
    ACPI_METHOD_ARGUMENT_V1 *arg = &params->Argument[0];

    // Loop through each remaining parameters and convert to correct type
    for(size_t i=0; i < params->ArgumentCount; i++) {
        char *carg = argv[i+CMD_MIN_ARG_COUNT];

        // Make sure this parameter will not overflow our buffer allocation
        size_t str_len = strlen(carg);
        if( ((UINT64)arg->Data - (UINT64)buffer.get()) + str_len > buffer_max ) {
            printf("Parameters too long\n");
            return ERROR_INVALID_PARAMETER;
        }
        
        // GUID must be in this exact format {25cb5207-ac36-427d-aaef-3aa78877d27e}
        if(carg[0] == '{') {
            int status = CharToGUID(arg->Data, 16, carg, str_len+1); // Include terminating \0 in length
            if(status != ERROR_SUCCESS) {
                printf("Failed to convert GUID\n");
                printf("Please provide GUID in this format: {25cb5207-ac36-427d-aaef-3aa78877d27e}\n");
                return status;
            }
            // Print out the GUID
            printf("Converted GUID: {");
            for(size_t j=0; j < 16; j++) {
                printf("0x%x,", arg->Data[j]);
            }
            printf("}\n");

            arg->Type = ACPI_METHOD_ARGUMENT_BUFFER;
            arg->DataLength = 16;

        } else if(carg[0] == '\'') {
            // Pull off the start and ending ' '
            arg->Type = ACPI_METHOD_ARGUMENT_STRING;
            arg->DataLength = static_cast<USHORT>(strlen(carg)-1);
            strncpy_s(reinterpret_cast<char*>(arg->Data), MAX_STRING_LEN, &carg[1], arg->DataLength-1);
            printf("Converting to String: %s\n", arg->Data);
        } else {
            char *endptr = nullptr;
            arg->Type = ACPI_METHOD_ARGUMENT_INTEGER;
            arg->DataLength = 4; // Length of DWORD
            arg->Argument = strtol(carg, &endptr, 0); // Try to guess the base
            if(endptr == carg) {
                printf("Failed to convert number\n");
                return ERROR_INVALID_PARAMETER;
            }
            printf("Converted to Number: 0x%x\n",arg->Argument);
        }

        params->Size += arg->DataLength;

        // Increment to next value
        arg = reinterpret_cast<ACPI_METHOD_ARGUMENT_V1*>(
            reinterpret_cast<UINT64>(arg) + sizeof(USHORT) * 2 + arg->DataLength);
    }

    // Evaluate and dump output
    return DumpAcpi(params);
}

/*
 * Function: int ReadRxBuffer
 *
 * Description:
 * The ReadRxBuffer function reads the receive buffer from a Kernel-Mode Driver Framework (KMDF) driver.
 * It sends a request to the driver and receives the buffer data.
 *
 * Parameters:
 * HANDLE hDevice: A handle to the device from which the receive buffer is to be read.
 *
 * Return Value:
 * Returns ERROR_SUCCESS if the receive buffer is successfully read, otherwise returns ERROR_INVALID_PARAMETER.
 */
#ifdef EC_TEST_SHARED_BUFFER
int ReadRxBuffer(
    _In_ HANDLE hDevice
    )
{
    BOOL bRc;
    ULONG bytesReturned;
    ULONG inbuf;
    RxBufferRsp_t rxrsp;

    printf("\nCalling DeviceIoControl IOCTL_GET_RX_BUFFER\n");

    bRc = DeviceIoControl ( hDevice,
                            (DWORD) IOCTL_READ_RX_BUFFER,
                            &inbuf,
                            sizeof(inbuf),
                            &rxrsp,
                            sizeof(rxrsp),
                            &bytesReturned,
                            NULL
                            );

    if ( !bRc )
    {
        printf ( "***       Error in DeviceIoControl : %d \n", GetLastError());
        return ERROR_INVALID_PARAMETER;
    }

    // Print out notification details
    printf("***                 data: 0x%llx\n", rxrsp.data);

    return ERROR_SUCCESS;
}
#endif // EC_TEST_SHARED_BUFFER

#ifdef EC_TEST_NOTIFICATIONS
/*
 * Function: DDWORD NotificationThread
 *
 * Description:
 * The NotificationThread function retrieves a notification from a Kernel-Mode Driver Framework (KMDF) driver.
 * It sends a request to the driver and receives the notification details.
 *
 * Parameters:
 * LPVOID lpParam: A handle to ready event we notfiy after sending IOCTL to wait for events
 *
 * Return Value:
 * Returns ERROR_SUCCESS if the notification is successfully retrieved, otherwise returns ERROR_INVALID_PARAMETER.
 */
DWORD WINAPI NotificationThread(LPVOID lpParam) 
{
    UNREFERENCED_PARAMETER(lpParam);

    // Main loop to wait for notifications
    for(;;) {
        UINT32 event = WaitForNotification(0);
        printf("Received Notification Event: 0x%x\n", event);
        // If we get exit event then break out of loop and exit thread
        if( WaitForSingleObject(gExitEvent, 0) == WAIT_OBJECT_0) {
            break;
        }
    }

    return 0;
}


/*
 * Function: HANDLE StartNotificationListener
 *
 * Description:
 * The StartNotificationListener function creates a thread to listen for notifications and returns a handle to the thread.
 * It creates an event to signal when the thread is ready and waits for the thread to be ready or terminated.
 *
 * Parameters:
 * None
 *
 * Return Value:
 * Returns a handle to the notification listener thread if successful, otherwise returns NULL.
 */
HANDLE StartNotificationListener(void)
{
    HANDLE hThread;
    DWORD dwThreadId;

    // Initialize external notification lib
    int status = InitializeNotification();
    if(status != ERROR_SUCCESS) {
        printf("InitializeNotification failed, status: 0x%x\n", status);
        return NULL;
    }

    // Create the thread
    hThread = CreateThread(
        NULL,                   // Default security attributes
        0,                      // Default stack size
        NotificationThread,     // Thread routine
        NULL,                   // Parameter to thread routine
        0,                      // Default creation flags
        &dwThreadId);           // Receive thread identifier

    return hThread;
}
#endif // EC_TEST_NOTIFICATIONS


/*
 * Function: int main
 *
 * Description:
 * The main function serves as the entry point for the program. It parses the command line arguments,
 * retrieves a handle to the KMDF driver, and evaluates an ACPI method on the device.
 *
 * Parameters:
 * int argc: The number of command line arguments.
 * char* argv[]: The array of command line arguments.
 *
 * Return Value:
 * Returns the status of the operations. Returns ERROR_SUCCESS if all operations are successful,
 * otherwise returns an error code.
 */
int __cdecl
main(
    _In_ int argc,
    _In_reads_(argc) char* argv[]
    )
{

    HANDLE hThread = NULL;
    int status = ERROR_SUCCESS;

    // Keep only one instance of the application running
    // This makes the App & Driver simple by not allowing multiple instances
    //
    HANDLE hMutex = CreateMutex(NULL, TRUE, L"Global\\ECTestAppMutex");
    if (hMutex == NULL) {
        status = GetLastError();
        printf("CreateMutex failed, error: %d\n", status);
        goto CleanUp;
    }

    if (GetLastError() == ERROR_ALREADY_EXISTS) {
        printf("Another instance of the application is already running.\n");
        goto CleanUp;
    }

#ifdef EC_TEST_NOTIFICATIONS
    // Create the exit event
    gExitEvent = CreateEvent(NULL, TRUE, FALSE, NULL);
    if (gExitEvent == NULL) {
        status = GetLastError();
        printf("CreateEvent failed, error: %d\n", status);
        goto CleanUp;
    }

    // This creates a new thread and blocks until listener is running
    hThread = StartNotificationListener();
    if(hThread == NULL) {
        goto CleanUp;
    }
#endif // EC_TEST_NOTIFICATIONS

    status = ParseCmdline(argc,argv);
    if(status != ERROR_SUCCESS) {
        goto CleanUp;
    }

    // Loop until we hit "q to quit"
    printf("Waiting for notification press 'q' to quit.\n");
    int key;
    for(;;) {
        key = getchar();
        if( key == 'q') {
            break;
        }
    }

    printf("You pressed 'q'. Exiting...\n");
CleanUp:

    // Signal the exit event to stop the thread
    if(gExitEvent) SetEvent(gExitEvent);
    if(hThread) CancelSynchronousIo(hThread);
    if(hThread) WaitForSingleObject(hThread, INFINITE);
    if(hThread) CleanupNotification();
    if(hThread) CloseHandle(hThread);

    if(gExitEvent) CloseHandle(gExitEvent);
    if(hMutex) CloseHandle(hMutex);

    return status;
}

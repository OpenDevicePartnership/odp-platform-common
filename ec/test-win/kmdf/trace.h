/*++
Module Name:
    trace.h

Abstract:
    Header file for the debug tracing related function definitions and macros.

Environment:
    Kernel mode
--*/

#include <evntrace.h> // For TRACE_LEVEL definitions

// Define the tracing flags.
// Tracing GUID - {2b869d11-4cbb-4080-b229-01f971dba7a8}


#define WPP_CONTROL_GUIDS \
    WPP_DEFINE_CONTROL_GUID( EcTestTraceGuid, (2b869d11,4cbb,4080,b229,01f971dba7a8), \
        WPP_DEFINE_BIT(TRACE_ALL)       /* bit  0 = 0x00000001 */  \
        WPP_DEFINE_BIT(TRACE_DRIVER)    /* bit  1 = 0x00000002 */\
        WPP_DEFINE_BIT(TRACE_DEVICE)    /* bit  2 = 0x00000004 */\
        WPP_DEFINE_BIT(TRACE_QUEUE)     /* bit  3 = 0x00000008 */\
        )

#define WPP_LEVEL_FLAGS_LOGGER(lvl,flags) WPP_LEVEL_LOGGER(flags)
#define WPP_LEVEL_FLAGS_ENABLED(lvl, flags) (WPP_LEVEL_ENABLED(flags) && WPP_CONTROL(WPP_BIT_ ## flags).Level  >= lvl)

// This comment block is scanned by the trace preprocessor to define our Trace function.
// begin_wpp config
// FUNC Trace(LEVEL, FLAGS, MSG, ...);
// end_wpp

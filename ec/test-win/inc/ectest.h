
// Define IOCTL's and structures shared between KMDF and Application
#define IOCTL_GET_NOTIFICATION 0x1
#define IOCTL_READ_RX_BUFFER 0x2

#define SBSAQEMU_SHARED_MEM_BASE 0x10060000000

typedef struct {
    UINT64 count;
    UINT64 timestamp;
    UINT32  lastevent;
} NotificationRsp_t;

typedef struct {
    UINT8 type;
} NotificationReq_t;

typedef struct {
    UINT64 data;
} RxBufferRsp_t;

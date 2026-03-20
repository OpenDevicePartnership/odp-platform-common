// SbsaQemuPlatform.h
// Definitions for mapping shared memory and RX/TX buffers with SP

#define SBSAQEMU_RESERVED_MEMORY_BASE 0x10060000000
#define SBSAQEMU_RESERVED_MEMORY_SIZE 0x100000 // Reserve 1MB

#define SBSAQEMU_SHARED_MEM_BASE 0x10060000000
#define SBSAQEMU_SHARED_MEM_PAGE_COUNT 0x8

#define SBSAQEMU_TX_BUFFER_BASE 0x10060080000
#define SBSAQEMU_RX_BUFFER_BASE 0x10060090000
#define EC_SVC_TX_BUFFER_BASE 0x100600A0000
#define EC_SVC_RX_BUFFER_BASE 0x100600B0000

#define EC_SERVICE_VMID 0x8002

// This just needs ot be unique value use ascii of SBSAQEMU
#define SBSAQEMU_SHARED_MEM_TAG 0x5342534151454D55
#define EC_SVC_MANAGEMENT_GUID_LO 0xfde54757330c1273
#define EC_SVC_MANAGEMENT_GUID_HI 0x3903750298195b65

// Commands to send to EC management service
#define EC_CAP_MAP_SHARE 0x5

#define FFA_VERSION_SMC 0x84000063
#define FFA_RXTX_MAP_SMC 0xC4000066
#define FFA_RXTX_UNMAP_SMC 0x84000067
#define FFA_MEM_SHARE_SMC 0x84000073
#define FFA_MSG_SEND_DIRECT_REQ2_SMC 0xC400008D

typedef UINT16 ffa_id_t;
typedef UINT32 ffa_memory_region_flags_t;
typedef UINT64 ffa_memory_handle_t;


typedef struct {
	UINT8 data_access : 2;
	UINT8 instruction_access : 2;
} ffa_memory_access_permissions_t;

struct ffa_memory_access_impdef {
	UINT64 val[2];
};

typedef struct {
	UINT64 address;
	UINT32 page_count;
	UINT32 reserved;
} memory_region_t;

typedef struct {
	UINT32 total_page_count;
	UINT32 address_range_count;
	UINT64 reserved;
	memory_region_t regions[1];
} composite_memory_region_t;

typedef struct {
  UINT16 id;
  UINT8 perm;
  UINT8 flags;
} ffa_memory_attributes_t;

typedef struct {
  UINT64 impl_def[2];
} ffa_memory_access_impdef_t;

typedef struct {
	ffa_memory_attributes_t receiver_permissions;
	/**
	 * Offset in bytes from the start of the outer `ffa_memory_region` to
	 * an `ffa_composite_memory_region` struct.
	 */
	UINT32 composite_memory_region_offset;
	//ffa_memory_access_impdef_t impldef;
  UINT64 reserved_0;
}ffa_memory_access_t;


typedef struct {
	/**
	 * The ID of the VM which originally sent the memory region, i.e. the
	 * owner.
	 */
	ffa_id_t sender;
	UINT16 attributes;
	/** Flags to control behaviour of the transaction. */
	ffa_memory_region_flags_t flags;
	ffa_memory_handle_t handle;
	/**
	 * An implementation defined value associated with the receiver and the
	 * memory region.
	 */
	UINT64 tag;
	/* Size of the memory access descriptor. */
	UINT32 memory_access_desc_size;
	/**
	 * The number of `ffa_memory_access` entries included in this
	 * transaction.
	 */
	UINT32 receiver_count;
	/**
	 * Offset of the 'ffa_memory_access' field, which relates to the memory access
	 * descriptors.
	 */
	UINT32 receivers_offset;
	/** Reserved field (12 bytes) must be 0. */
	UINT32 reserved[3];

//	ffa_memory_access_t memory_access;
//	composite_memory_region_t memory_region;
} ffa_memory_region_t;
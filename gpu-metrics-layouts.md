# Linux AMDGPU GPU-metrics binary layouts

## Provenance and ABI assumptions

**Upstream:** `torvalds/linux`, commit `2be02a7c996aa733bb36e29e07715621b0de9736` (2026-08-21), `drivers/gpu/drm/amd/include/kgd_pp_interface.h`.

These definitions use the kernel's `uint*_t` spellings exactly. The header has **no** `#pragma pack`, `__packed`, or `__attribute__((packed))` annotation. The offsets and sizes below are therefore the results of normal C layout on the Linux x86-64 ABI (natural member alignment, `uint16_t`/`uint32_t`/`uint64_t` alignment 2/4/8; tail rounded to the struct's maximum alignment). Do not impose packed layout. The public table is a native-endian kernel/userspace ABI; on the Linux hosts concerned it is little-endian.

Header constants used by arrays:

```c
#define NUM_HBM_INSTANCES 4
#define NUM_XGMI_LINKS    8
#define MAX_GFX_CLKS      8
#define MAX_CLKS          4
#define NUM_VCN           4
#define NUM_JPEG_ENG      32
#define NUM_JPEG_ENG_V1   40
#define MAX_XCC           8
#define NUM_XCP           8
```

## Common header and revision selection

```c
struct metrics_table_header {
        uint16_t structure_size;   // offset 0, 2 bytes
        uint8_t  format_revision;  // offset 2, 1 byte
        uint8_t  content_revision; // offset 3, 1 byte
}; // sizeof 4
```

`structure_size` is a native-endian 16-bit byte count of the emitted table; `format_revision` and `content_revision` select the matching `gpu_metrics_v<format>_<content>` definition. The kernel's `smu_cmn_init_soft_gpu_metrics()` first fills the complete chosen struct with `0xFF`, then sets `format_revision`, `content_revision`, and `structure_size = sizeof(*tmp)`. Thus read and validate all three header bytes/words before decoding an exact-size fixed-layout table.

## v1 layouts

### `struct gpu_metrics_v1_3` — 120 bytes

```c
struct gpu_metrics_v1_3 {
        struct metrics_table_header common_header;       // 0
        /* Temperature */
        uint16_t temperature_edge;                       // 4
        uint16_t temperature_hotspot;                    // 6
        uint16_t temperature_mem;                        // 8
        uint16_t temperature_vrgfx;                      // 10
        uint16_t temperature_vrsoc;                      // 12
        uint16_t temperature_vrmem;                      // 14
        /* Utilization */
        uint16_t average_gfx_activity;                   // 16
        uint16_t average_umc_activity; // memory controller // 18
        uint16_t average_mm_activity; // UVD or VCN       // 20
        /* Power/Energy */
        uint16_t average_socket_power;                   // 22
        uint64_t energy_accumulator;                     // 24
        /* Driver attached timestamp (in ns) */
        uint64_t system_clock_counter;                   // 32
        /* Average clocks */
        uint16_t average_gfxclk_frequency;               // 40
        uint16_t average_socclk_frequency;               // 42
        uint16_t average_uclk_frequency;                 // 44
        uint16_t average_vclk0_frequency;                // 46
        uint16_t average_dclk0_frequency;                // 48
        uint16_t average_vclk1_frequency;                // 50
        uint16_t average_dclk1_frequency;                // 52
        /* Current clocks */
        uint16_t current_gfxclk;                         // 54
        uint16_t current_socclk;                         // 56
        uint16_t current_uclk;                           // 58
        uint16_t current_vclk0;                          // 60
        uint16_t current_dclk0;                          // 62
        uint16_t current_vclk1;                          // 64
        uint16_t current_dclk1;                          // 66
        /* Throttle status */
        uint32_t throttle_status;                        // 68
        /* Fans */
        uint16_t current_fan_speed;                      // 72
        /* Link width/speed */
        uint16_t pcie_link_width;                        // 74
        uint16_t pcie_link_speed; // in 0.1 GT/s          // 76
        uint16_t padding;                                // 78
        uint32_t gfx_activity_acc;                       // 80
        uint32_t mem_activity_acc;                       // 84
        uint16_t temperature_hbm[NUM_HBM_INSTANCES];     // 88, 4
        /* PMFW attached timestamp (10ns resolution) */
        uint64_t firmware_timestamp;                     // 96
        /* Voltage (mV) */
        uint16_t voltage_soc;                            // 104
        uint16_t voltage_gfx;                            // 106
        uint16_t voltage_mem;                            // 108
        uint16_t padding1;                               // 110
        /* Throttle status (ASIC independent) */
        uint64_t indep_throttle_status;                  // 112
};
```

### `struct gpu_metrics_v1_4` — 288 bytes

```c
struct gpu_metrics_v1_4 {
        struct metrics_table_header common_header;       // 0
        /* Temperature (Celsius) */
        uint16_t temperature_hotspot;                    // 4
        uint16_t temperature_mem;                        // 6
        uint16_t temperature_vrsoc;                      // 8
        /* Power (Watts) */
        uint16_t curr_socket_power;                      // 10
        /* Utilization (%) */
        uint16_t average_gfx_activity;                   // 12
        uint16_t average_umc_activity; // memory controller // 14
        uint16_t vcn_activity[NUM_VCN];                  // 16, 4
        /* Energy (15.259uJ (2^-16) units) */
        uint64_t energy_accumulator;                     // 24
        /* Driver attached timestamp (in ns) */
        uint64_t system_clock_counter;                   // 32
        /* Throttle status */
        uint32_t throttle_status;                        // 40
        /* Clock Lock Status. Each bit corresponds to clock instance */
        uint32_t gfxclk_lock_status;                     // 44
        /* Link width (number of lanes) and speed (in 0.1 GT/s) */
        uint16_t pcie_link_width;                        // 48
        uint16_t pcie_link_speed;                        // 50
        /* XGMI bus width and bitrate (in Gbps) */
        uint16_t xgmi_link_width;                        // 52
        uint16_t xgmi_link_speed;                        // 54
        /* Utilization Accumulated (%) */
        uint32_t gfx_activity_acc;                       // 56
        uint32_t mem_activity_acc;                       // 60
        /*PCIE accumulated bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_acc;                     // 64
        /*PCIE instantaneous bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_inst;                    // 72
        /* PCIE L0 to recovery state transition accumulated count */
        uint64_t pcie_l0_to_recov_count_acc;             // 80
        /* PCIE replay accumulated count */
        uint64_t pcie_replay_count_acc;                  // 88
        /* PCIE replay rollover accumulated count */
        uint64_t pcie_replay_rover_count_acc;            // 96
        /* XGMI accumulated data transfer size(KiloBytes) */
        uint64_t xgmi_read_data_acc[NUM_XGMI_LINKS];     // 104, 8
        uint64_t xgmi_write_data_acc[NUM_XGMI_LINKS];    // 168, 8
        /* PMFW attached timestamp (10ns resolution) */
        uint64_t firmware_timestamp;                     // 232
        /* Current clocks (Mhz) */
        uint16_t current_gfxclk[MAX_GFX_CLKS];           // 240, 8
        uint16_t current_socclk[MAX_CLKS];               // 256, 4
        uint16_t current_vclk0[MAX_CLKS];                // 264, 4
        uint16_t current_dclk0[MAX_CLKS];                // 272, 4
        uint16_t current_uclk;                           // 280
        uint16_t padding;                                // 282
};
```

### `struct gpu_metrics_v1_5` — 328 bytes

```c
struct gpu_metrics_v1_5 {
        struct metrics_table_header common_header;       // 0
        /* Temperature (Celsius) */
        uint16_t temperature_hotspot;                    // 4
        uint16_t temperature_mem;                        // 6
        uint16_t temperature_vrsoc;                      // 8
        /* Power (Watts) */
        uint16_t curr_socket_power;                      // 10
        /* Utilization (%) */
        uint16_t average_gfx_activity;                   // 12
        uint16_t average_umc_activity; // memory controller // 14
        uint16_t vcn_activity[NUM_VCN];                  // 16, 4
        uint16_t jpeg_activity[NUM_JPEG_ENG];            // 24, 32
        /* Energy (15.259uJ (2^-16) units) */
        uint64_t energy_accumulator;                     // 56
        /* Driver attached timestamp (in ns) */
        uint64_t system_clock_counter;                   // 64
        /* Throttle status */
        uint32_t throttle_status;                        // 72
        /* Clock Lock Status. Each bit corresponds to clock instance */
        uint32_t gfxclk_lock_status;                     // 76
        /* Link width (number of lanes) and speed (in 0.1 GT/s) */
        uint16_t pcie_link_width;                        // 80
        uint16_t pcie_link_speed;                        // 82
        /* XGMI bus width and bitrate (in Gbps) */
        uint16_t xgmi_link_width;                        // 84
        uint16_t xgmi_link_speed;                        // 86
        /* Utilization Accumulated (%) */
        uint32_t gfx_activity_acc;                       // 88
        uint32_t mem_activity_acc;                       // 92
        /*PCIE accumulated bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_acc;                     // 96
        /*PCIE instantaneous bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_inst;                    // 104
        /* PCIE L0 to recovery state transition accumulated count */
        uint64_t pcie_l0_to_recov_count_acc;             // 112
        /* PCIE replay accumulated count */
        uint64_t pcie_replay_count_acc;                  // 120
        /* PCIE replay rollover accumulated count */
        uint64_t pcie_replay_rover_count_acc;            // 128
        /* PCIE NAK sent accumulated count */
        uint32_t pcie_nak_sent_count_acc;                // 136
        /* PCIE NAK received accumulated count */
        uint32_t pcie_nak_rcvd_count_acc;                // 140
        /* XGMI accumulated data transfer size(KiloBytes) */
        uint64_t xgmi_read_data_acc[NUM_XGMI_LINKS];     // 144, 8
        uint64_t xgmi_write_data_acc[NUM_XGMI_LINKS];    // 208, 8
        /* PMFW attached timestamp (10ns resolution) */
        uint64_t firmware_timestamp;                     // 272
        /* Current clocks (Mhz) */
        uint16_t current_gfxclk[MAX_GFX_CLKS];           // 280, 8
        uint16_t current_socclk[MAX_CLKS];               // 296, 4
        uint16_t current_vclk0[MAX_CLKS];                // 304, 4
        uint16_t current_dclk0[MAX_CLKS];                // 312, 4
        uint16_t current_uclk;                           // 320
        uint16_t padding;                                // 322
};
```

### Supporting XCP element layouts used by v1_6–v1_8

```c
struct amdgpu_xcp_metrics {                              // sizeof 168
        uint32_t gfx_busy_inst[MAX_XCC];                 // 0, 8
        uint16_t jpeg_busy[NUM_JPEG_ENG];                // 32, 32
        uint16_t vcn_busy[NUM_VCN];                      // 96, 4
        uint64_t gfx_busy_acc[MAX_XCC];                  // 104, 8
};
struct amdgpu_xcp_metrics_v1_1 {                         // sizeof 232
        uint32_t gfx_busy_inst[MAX_XCC];                 // 0, 8
        uint16_t jpeg_busy[NUM_JPEG_ENG];                // 32, 32
        uint16_t vcn_busy[NUM_VCN];                      // 96, 4
        uint64_t gfx_busy_acc[MAX_XCC];                  // 104, 8
        uint64_t gfx_below_host_limit_acc[MAX_XCC];      // 168, 8
};
struct amdgpu_xcp_metrics_v1_2 {                         // sizeof 376
        uint32_t gfx_busy_inst[MAX_XCC];                 // 0, 8
        uint16_t jpeg_busy[NUM_JPEG_ENG_V1];             // 32, 40
        uint16_t vcn_busy[NUM_VCN];                      // 112, 4
        uint64_t gfx_busy_acc[MAX_XCC];                  // 120, 8
        uint64_t gfx_below_host_limit_ppt_acc[MAX_XCC];  // 184, 8
        uint64_t gfx_below_host_limit_thm_acc[MAX_XCC];  // 248, 8
        uint64_t gfx_low_utilization_acc[MAX_XCC];       // 312, 8
};
```

### `struct gpu_metrics_v1_6` — 1,664 bytes

```c
struct gpu_metrics_v1_6 {
        struct metrics_table_header common_header;       // 0
        /* Temperature (Celsius) */
        uint16_t temperature_hotspot;                    // 4
        uint16_t temperature_mem;                        // 6
        uint16_t temperature_vrsoc;                      // 8
        /* Power (Watts) */
        uint16_t curr_socket_power;                      // 10
        /* Utilization (%) */
        uint16_t average_gfx_activity;                   // 12
        uint16_t average_umc_activity; // memory controller // 14
        /* Energy (15.259uJ (2^-16) units) */
        uint64_t energy_accumulator;                     // 16
        /* Driver attached timestamp (in ns) */
        uint64_t system_clock_counter;                   // 24
        /* Accumulation cycle counter */
        uint32_t accumulation_counter;                   // 32
        /* Accumulated throttler residencies */
        uint32_t prochot_residency_acc;                  // 36
        uint32_t ppt_residency_acc;                      // 40
        uint32_t socket_thm_residency_acc;               // 44
        uint32_t vr_thm_residency_acc;                   // 48
        uint32_t hbm_thm_residency_acc;                  // 52
        /* Clock Lock Status. Each bit corresponds to clock instance */
        uint32_t gfxclk_lock_status;                     // 56
        /* Link width (number of lanes) and speed (in 0.1 GT/s) */
        uint16_t pcie_link_width;                        // 60
        uint16_t pcie_link_speed;                        // 62
        /* XGMI bus width and bitrate (in Gbps) */
        uint16_t xgmi_link_width;                        // 64
        uint16_t xgmi_link_speed;                        // 66
        /* Utilization Accumulated (%) */
        uint32_t gfx_activity_acc;                       // 68
        uint32_t mem_activity_acc;                       // 72
        /*PCIE accumulated bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_acc;                     // 80
        /*PCIE instantaneous bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_inst;                    // 88
        /* PCIE L0 to recovery state transition accumulated count */
        uint64_t pcie_l0_to_recov_count_acc;             // 96
        /* PCIE replay accumulated count */
        uint64_t pcie_replay_count_acc;                  // 104
        /* PCIE replay rollover accumulated count */
        uint64_t pcie_replay_rover_count_acc;            // 112
        /* PCIE NAK sent accumulated count */
        uint32_t pcie_nak_sent_count_acc;                // 120
        /* PCIE NAK received accumulated count */
        uint32_t pcie_nak_rcvd_count_acc;                // 124
        /* XGMI accumulated data transfer size(KiloBytes) */
        uint64_t xgmi_read_data_acc[NUM_XGMI_LINKS];     // 128, 8
        uint64_t xgmi_write_data_acc[NUM_XGMI_LINKS];    // 192, 8
        /* PMFW attached timestamp (10ns resolution) */
        uint64_t firmware_timestamp;                     // 256
        /* Current clocks (Mhz) */
        uint16_t current_gfxclk[MAX_GFX_CLKS];           // 264, 8
        uint16_t current_socclk[MAX_CLKS];               // 280, 4
        uint16_t current_vclk0[MAX_CLKS];                // 288, 4
        uint16_t current_dclk0[MAX_CLKS];                // 296, 4
        uint16_t current_uclk;                           // 304
        /* Number of current partition */
        uint16_t num_partition;                          // 306
        /* XCP metrics stats */
        struct amdgpu_xcp_metrics xcp_stats[NUM_XCP];    // 312, 8
        /* PCIE other end recovery counter */
        uint32_t pcie_lc_perf_other_end_recovery;        // 1656
};
```

### `struct gpu_metrics_v1_7` — 2,208 bytes

```c
struct gpu_metrics_v1_7 {
        struct metrics_table_header common_header;       // 0
        /* Temperature (Celsius) */
        uint16_t temperature_hotspot;                    // 4
        uint16_t temperature_mem;                        // 6
        uint16_t temperature_vrsoc;                      // 8
        /* Power (Watts) */
        uint16_t curr_socket_power;                      // 10
        /* Utilization (%) */
        uint16_t average_gfx_activity;                   // 12
        uint16_t average_umc_activity; // memory controller // 14
        /* VRAM max bandwidthi (in GB/sec) at max memory clock */
        uint64_t mem_max_bandwidth;                      // 16
        /* Energy (15.259uJ (2^-16) units) */
        uint64_t energy_accumulator;                     // 24
        /* Driver attached timestamp (in ns) */
        uint64_t system_clock_counter;                   // 32
        /* Accumulation cycle counter */
        uint32_t accumulation_counter;                   // 40
        /* Accumulated throttler residencies */
        uint32_t prochot_residency_acc;                  // 44
        uint32_t ppt_residency_acc;                      // 48
        uint32_t socket_thm_residency_acc;               // 52
        uint32_t vr_thm_residency_acc;                   // 56
        uint32_t hbm_thm_residency_acc;                  // 60
        /* Clock Lock Status. Each bit corresponds to clock instance */
        uint32_t gfxclk_lock_status;                     // 64
        /* Link width (number of lanes) and speed (in 0.1 GT/s) */
        uint16_t pcie_link_width;                        // 68
        uint16_t pcie_link_speed;                        // 70
        /* XGMI bus width and bitrate (in Gbps) */
        uint16_t xgmi_link_width;                        // 72
        uint16_t xgmi_link_speed;                        // 74
        /* Utilization Accumulated (%) */
        uint32_t gfx_activity_acc;                       // 76
        uint32_t mem_activity_acc;                       // 80
        /*PCIE accumulated bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_acc;                     // 88
        /*PCIE instantaneous bandwidth (GB/sec) */
        uint64_t pcie_bandwidth_inst;                    // 96
        /* PCIE L0 to recovery state transition accumulated count */
        uint64_t pcie_l0_to_recov_count_acc;             // 104
        /* PCIE replay accumulated count */
        uint64_t pcie_replay_count_acc;                  // 112
        /* PCIE replay rollover accumulated count */
        uint64_t pcie_replay_rover_count_acc;            // 120
        /* PCIE NAK sent accumulated count */
        uint32_t pcie_nak_sent_count_acc;                // 128
        /* PCIE NAK received accumulated count */
        uint32_t pcie_nak_rcvd_count_acc;                // 132
        /* XGMI accumulated data transfer size(KiloBytes) */
        uint64_t xgmi_read_data_acc[NUM_XGMI_LINKS];     // 136, 8
        uint64_t xgmi_write_data_acc[NUM_XGMI_LINKS];    // 200, 8
        /* XGMI link status(active/inactive) */
        uint16_t xgmi_link_status[NUM_XGMI_LINKS];       // 264, 8
        uint16_t padding;                                // 280
        /* PMFW attached timestamp (10ns resolution) */
        uint64_t firmware_timestamp;                     // 288
        /* Current clocks (Mhz) */
        uint16_t current_gfxclk[MAX_GFX_CLKS];           // 296, 8
        uint16_t current_socclk[MAX_CLKS];               // 312, 4
        uint16_t current_vclk0[MAX_CLKS];                // 320, 4
        uint16_t current_dclk0[MAX_CLKS];                // 328, 4
        uint16_t current_uclk;                           // 336
        /* Number of current partition */
        uint16_t num_partition;                          // 338
        /* XCP metrics stats */
        struct amdgpu_xcp_metrics_v1_1 xcp_stats[NUM_XCP]; // 344, 8
        /* PCIE other end recovery counter */
        uint32_t pcie_lc_perf_other_end_recovery;        // 2200
};
```

### `struct gpu_metrics_v1_8` — 3,360 bytes

`gpu_metrics_v1_8` is declaration-identical to v1_7 through `num_partition` (offset 338), and has the following final two declarations in place of v1_7's final two declarations. This is verbatim-equivalent without needlessly repeating the identical 0–339 byte prefix:

```c
        /* XCP metrics stats */
        struct amdgpu_xcp_metrics_v1_2 xcp_stats[NUM_XCP]; // offset 344, 8
        /* PCIE other end recovery counter */
        uint32_t pcie_lc_perf_other_end_recovery;          // offset 3352
};
```

The v1_8 specific element type is fully declared above; its 376-byte elements make the `xcp_stats` array 3,008 bytes.

### v1.4–v1.8 compatibility map

All v1.4–v1.8 declarations contain: `common_header`; `temperature_hotspot`, `temperature_mem`, `temperature_vrsoc`; `curr_socket_power`; `average_gfx_activity`, `average_umc_activity`; `energy_accumulator`; `system_clock_counter`; `gfxclk_lock_status`; PCIe/XGMI link width/speed; `gfx_activity_acc`, `mem_activity_acc`; PCIe bandwidth/L0-recovery/replay/replay-rollover counters; XGMI read/write data arrays; `firmware_timestamp`; current GFX/SOC/VCLK0/DCLK0/UCLK clocks; and a named `padding` field. However, they are **not append-only ABI revisions**.

* Offsets 0–15 are identical in all five: header 0, temperatures 4/6/8, power 10, activities 12/14.
* From offset 16 they diverge: v1.4 has `vcn_activity` and energy at 24; v1.5 also has `jpeg_activity` and energy at 56; v1.6 starts energy at 16; v1.7 and v1.8 insert `mem_max_bandwidth` at 16 and energy at 24.
* v1.4/v1.5 have `throttle_status`; v1.6–v1.8 instead expose accumulation and throttler-residency fields. v1.5–v1.8 have NAK counters. v1.6–v1.8 add XCP/partition data; v1.7/v1.8 add `mem_max_bandwidth` and XGMI link status. Always decode only the exact header revision.

### `struct gpu_metrics_v1_9` — dynamic, not a fixed wire layout

```c
struct gpu_metrics_attr {
        /* Field type encoded with AMDGPU_METRICS_ENC_ATTR */
        uint64_t attr_encoding; // offset 0
        /* Attribute value, depends on attr_encoding */
        void *attr_value;       // offset 8 (8-byte native pointer on x86-64)
}; // sizeof 16 on x86-64

struct gpu_metrics_v1_9 {
        struct metrics_table_header common_header; // offset 0
        int attr_count;                            // offset 4
        struct gpu_metrics_attr metrics_attrs[];  // flexible array, offset 8
}; // sizeof 8 on x86-64; alignment 8
```

v1_9 is dynamic/pointer-based: `metrics_attrs[]` is a C flexible array whose count is `attr_count`, and every attribute's `attr_value` is a native kernel pointer. It is not a self-contained byte-offset telemetry table suitable for parsing as a sysfs raw blob across the kernel/userspace boundary. Its `attr_encoding` uses `AMDGPU_METRICS_ENC_ATTR(unit,type,id,inst)` (unit bits 24–31, type 20–23, id 10–19, instance 0–9); its type determines the pointed-to value width.

## v2 APU layouts

### `struct gpu_metrics_v2_1` — 120 bytes

```c
struct gpu_metrics_v2_1 {
        struct metrics_table_header common_header;       // 0
        /* Temperature */
        uint16_t temperature_gfx; // gfx temperature on APUs // 4
        uint16_t temperature_soc; // soc temperature on APUs // 6
        uint16_t temperature_core[8]; // CPU core temperature on APUs // 8, 8
        uint16_t temperature_l3[2];                     // 24, 2
        /* Utilization */
        uint16_t average_gfx_activity;                  // 28
        uint16_t average_mm_activity; // UVD or VCN      // 30
        /* Driver attached timestamp (in ns) */
        uint64_t system_clock_counter;                  // 32
        /* Power/Energy */
        uint16_t average_socket_power; // dGPU + APU power on A + A platform // 40
        uint16_t average_cpu_power;                     // 42
        uint16_t average_soc_power;                     // 44
        uint16_t average_gfx_power;                     // 46
        uint16_t average_core_power[8]; // CPU core power on APUs // 48, 8
        /* Average clocks */
        uint16_t average_gfxclk_frequency;              // 64
        uint16_t average_socclk_frequency;              // 66
        uint16_t average_uclk_frequency;                // 68
        uint16_t average_fclk_frequency;                // 70
        uint16_t average_vclk_frequency;                // 72
        uint16_t average_dclk_frequency;                // 74
        /* Current clocks */
        uint16_t current_gfxclk;                        // 76
        uint16_t current_socclk;                        // 78
        uint16_t current_uclk;                          // 80
        uint16_t current_fclk;                          // 82
        uint16_t current_vclk;                          // 84
        uint16_t current_dclk;                          // 86
        uint16_t current_coreclk[8]; // CPU core clocks  // 88, 8
        uint16_t current_l3clk[2];                      // 104, 2
        /* Throttle status */
        uint32_t throttle_status;                       // 108
        /* Fans */
        uint16_t fan_pwm;                               // 112
        uint16_t padding[3];                            // 114, 3
};
```

### `struct gpu_metrics_v2_2` — 128 bytes

v2_2 is declaration-identical to v2_1 through `uint16_t padding[3];` at offset 114, then appends:

```c
        /* Throttle status (ASIC independent) */
        uint64_t indep_throttle_status;                 // offset 120
};
```

### `struct gpu_metrics_v2_3` — 152 bytes

v2_3 is declaration-identical to v2_2, then appends:

```c
        /* Average Temperature */
        uint16_t average_temperature_gfx; // average gfx temperature on APUs // 128
        uint16_t average_temperature_soc; // average soc temperature on APUs // 130
        uint16_t average_temperature_core[8]; // average CPU core temperature on APUs // 132, 8
        uint16_t average_temperature_l3[2];   // 148, 2
};
```

### `struct gpu_metrics_v2_4` — 168 bytes

v2_4 has an otherwise declaration-identical v2_3 layout and offsets, but the upstream comments explicitly specify scales for the shared fields. Its full additional tail is:

```c
        /* Power/Voltage (unit: mV) */
        uint16_t average_cpu_voltage;                    // 152
        uint16_t average_soc_voltage;                    // 154
        uint16_t average_gfx_voltage;                    // 156
        /* Power/Current (unit: mA) */
        uint16_t average_cpu_current;                    // 158
        uint16_t average_soc_current;                    // 160
        uint16_t average_gfx_current;                    // 162
};
```

For v2_4, upstream annotates the inherited groups as: `temperature_*` and `average_temperature_*`: **centi-Celsius**; `average_gfx_activity` and `average_mm_activity`: **centi** (centi-percent); power fields including `average_socket_power`: **mW**; all average/current clock fields: **MHz**; voltage tail: **mV**; current tail: **mA**. The preceding v2_1–v2_3 comments do **not** state units for their temperature/utilization/power groups, so do not retroactively assign the v2_4 scales without knowing the content revision.

## `struct gpu_metrics_v3_0` — 256 bytes

```c
struct gpu_metrics_v3_0 {
        struct metrics_table_header common_header;       // 0
        /* Temperature */
        /* gfx temperature on APUs */
        uint16_t temperature_gfx;                        // 4
        /* soc temperature on APUs */
        uint16_t temperature_soc;                        // 6
        /* CPU core temperature on APUs */
        uint16_t temperature_core[16];                   // 8, 16
        /* skin temperature on APUs */
        uint16_t temperature_skin;                       // 40
        /* Utilization */
        /* time filtered GFX busy % [0-100] */
        uint16_t average_gfx_activity;                   // 42
        /* time filtered VCN busy % [0-100] */
        uint16_t average_vcn_activity;                   // 44
        /* time filtered IPU per-column busy % [0-100] */
        uint16_t average_ipu_activity[8];                // 46, 8
        /* time filtered per-core C0 residency % [0-100] */
        uint16_t average_core_c0_activity[16];           // 62, 16
        /* time filtered DRAM read bandwidth [MB/sec] */
        uint16_t average_dram_reads;                     // 94
        /* time filtered DRAM write bandwidth [MB/sec] */
        uint16_t average_dram_writes;                    // 96
        /* time filtered IPU read bandwidth [MB/sec] */
        uint16_t average_ipu_reads;                      // 98
        /* time filtered IPU write bandwidth [MB/sec] */
        uint16_t average_ipu_writes;                     // 100
        /* implicit C alignment padding */               // 102–103
        /* Driver attached timestamp (in ns) */
        uint64_t system_clock_counter;                   // 104
        /* Power/Energy */
        /* time filtered power used for PPT/STAPM [APU+dGPU] [mW] */
        uint32_t average_socket_power;                   // 112
        /* time filtered IPU power [mW] */
        uint16_t average_ipu_power;                      // 116
        /* implicit C alignment padding */               // 118–119
        /* time filtered APU power [mW] */
        uint32_t average_apu_power;                      // 120
        /* time filtered GFX power [mW] */
        uint32_t average_gfx_power;                      // 124
        /* time filtered dGPU power [mW] */
        uint32_t average_dgpu_power;                     // 128
        /* time filtered sum of core power across all cores in the socket [mW] */
        uint32_t average_all_core_power;                 // 132
        /* calculated core power [mW] */
        uint16_t average_core_power[16];                 // 136, 16
        /* time filtered total system power [mW] */
        uint16_t average_sys_power;                      // 168
        /* maximum IRM defined STAPM power limit [mW] */
        uint16_t stapm_power_limit;                      // 170
        /* time filtered STAPM power limit [mW] */
        uint16_t current_stapm_power_limit;              // 172
        /* time filtered clocks [MHz] */
        uint16_t average_gfxclk_frequency;               // 174
        uint16_t average_socclk_frequency;               // 176
        uint16_t average_vpeclk_frequency;               // 178
        uint16_t average_ipuclk_frequency;               // 180
        uint16_t average_fclk_frequency;                 // 182
        uint16_t average_vclk_frequency;                 // 184
        uint16_t average_uclk_frequency;                 // 186
        uint16_t average_mpipu_frequency;                // 188
        /* Current clocks */
        /* target core frequency [MHz] */
        uint16_t current_coreclk[16];                    // 190, 16
        /* CCLK frequency limit enforced on classic cores [MHz] */
        uint16_t current_core_maxfreq;                   // 222
        /* GFXCLK frequency limit enforced on GFX [MHz] */
        uint16_t current_gfx_maxfreq;                    // 224
        /* Throttle Residency (ASIC dependent) */
        uint32_t throttle_residency_prochot;             // 228
        uint32_t throttle_residency_spl;                 // 232
        uint32_t throttle_residency_fppt;                // 236
        uint32_t throttle_residency_sppt;                // 240
        uint32_t throttle_residency_thm_core;            // 244
        uint32_t throttle_residency_thm_gfx;             // 248
        uint32_t throttle_residency_thm_soc;             // 252
        /* Metrics table alpha filter time constant [us] */
        uint32_t time_filter_alphavalue;                 // 256
};
```

**Correction note for the displayed v3 table:** normal C layout means after `current_stapm_power_limit` at 172–173, `average_gfxclk_frequency` is at **174** (not 172); the clock block runs 174–189; `current_coreclk` runs 190–221; current limits are 222/224; an implicit two-byte pad is 226–227; throttle fields are 228–255; `time_filter_alphavalue` is **256–259**; hence the actual `sizeof(struct gpu_metrics_v3_0)` is **264**, not 256. This correction is the authoritative v3 offset result.

## Unavailable-field sentinel

The in-tree producer initialization is the concrete convention: it performs `memset(header, 0xFF, sizeof(*tmp))` over the selected metrics struct before setting the three header fields. Therefore an unavailable/unpopulated unsigned metric retains an all-ones sentinel of its own declared width: `uint8_t 0xFF`, `uint16_t 0xFFFF`, `uint32_t 0xFFFFFFFF`, `uint64_t 0xFFFFFFFFFFFFFFFF`. Arrays have the same sentinel per element. `structure_size`, `format_revision`, and `content_revision` are overwritten with valid values.

This is a producer convention, not an explicit per-field range contract in `kgd_pp_interface.h`: a decoder should reject the all-ones value before applying a field's unit conversion, but should use revision-aware knowledge for fields where all-ones might ever be valid data. v1_9 pointers are not numeric sentinel values.

## Key unit reference

* v1.4–v1.8: `temperature_hotspot` is Celsius; `curr_socket_power` is Watts; `average_gfx_activity`/`average_umc_activity` are percent; `energy_accumulator` is 15.259 µJ (`2^-16`) units; `system_clock_counter` is driver-attached nanoseconds; `current_gfxclk` arrays are MHz; PCIe link speed is 0.1 GT/s; XGMI bitrate is Gbps; `gfxclk_lock_status`, `throttle_status`, and `indep_throttle_status` are status bitfields (the header gives no numeric unit). v1.3 has a `current_gfxclk` scalar but no unit comment; its `average_gfxclk_frequency` also has no unit comment. Do not infer a unit from similarly named later fields.
* v1.3: `firmware_timestamp` has 10 ns resolution; voltages are mV. `throttle_status` is ASIC-dependent and `indep_throttle_status` is ASIC-independent.
* v2.1–v2.3: `system_clock_counter` is nanoseconds; the header labels groups but does not specify numerical scales. v2.4 explicitly adds the centi-Celsius, centi, mW, and MHz labels described above.
* v3.0: comments explicitly give activity percentage `[0-100]`, traffic MB/sec, power mW, clocks MHz, timestamp ns, and filter time constant µs. It has throttle *residency* values but provides no unit for their counters.

## Source locations

* Header/common constants/nested XCP structs: `kgd_pp_interface.h` lines 355–410 and 531–535.
* v1.3 through v1.9 definitions: lines 874–1397.
* v2.1 through v3.0 definitions and explicit v2.4/v3 units: lines 1453–1773.
* Sentinel/revision initialization: `drivers/gpu/drm/amd/pm/swsmu/smu_cmn.h` lines 51–61.

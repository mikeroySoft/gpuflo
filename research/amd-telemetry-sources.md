# AMD GPU telemetry sources for gruflo (Linux / ROCm)

Research note for [issue #8 — Inventory AMD telemetry sources and support](https://github.com/michaelroy-amd/gruflo/issues/8).

**Question.** Which Linux ROCm telemetry interfaces should gruflo use for each candidate metric across all
`amd-smi`-supported AMD GPUs, considering `amd-smi` JSON commands, AMD SMI libraries, sysfs/hwmon/debugfs,
permissions, sampling cost, schema/version stability, process attribution, partitioned devices, and
source-reported fault/throttle/health signals?

---

## 0. How to read this file, and what was verified

Every claim below is tagged:

- **[F]** — **Fact** traced to a primary source (AMD's own docs, the AMD SMI header/source, or the Linux
  kernel `amdgpu`/`amdkfd` docs and source). A direct URL is given.
- **[I]** — **Inference** drawn from those facts by the author of this note. Not stated by any source.
- **[R]** — **Recommendation** for gruflo. Opinion, not fact.
- **[?]** — **Open question** that cannot be answered without an AMD GPU in hand.

**Verification status: no hardware was available.** The research host has no ROCm install and no AMD GPU
(`/opt/rocm*` absent, `/sys/class/kfd/` absent, `/sys/class/drm/` contains only `version`). Nothing here was
observed at runtime; every fact comes from published documentation or source code. Section 12 lists what must
be re-checked on real MI-series, RDNA, and APU hardware before the metric contract (issue #5), sampling
contract (issue #3), and process overlay contract (issue #4) are frozen.

**Source snapshot.** Docs read from the ROCm docs site at AMD SMI **26.5.0 / 27.0.0 / ROCm 7.14.0** vintage;
source read from `ROCm/rocm-systems` `develop` and `torvalds/linux` `master`, both fetched 2026-08-20.
The standalone `ROCm/amdsmi` repository is deprecated; the canonical source now lives in
[`ROCm/rocm-systems/projects/amdsmi`](https://github.com/ROCm/rocm-systems/tree/develop/projects/amdsmi). **[F]**
([ROCm/amdsmi README](https://github.com/ROCm/amdsmi))

---

## 1. Hardware span: what "all `amd-smi`-supported GPUs" actually means

**[F]** AMD SMI supports "AMD GPUs on Linux bare metal systems", "AMD GPUs in Linux virtual machine guests",
and AMD EPYC CPUs via `esmi_ib_library`; its GPU support set is defined as "AMD ROCm supported platforms".
([install](https://rocm.docs.amd.com/projects/amdsmi/en/latest/install/install.html))

**[F]** The ROCm compatibility matrix enumerates three device families — Instinct (MI100 `gfx908`, MI210/MI250/MI250X
`gfx90a`, MI300A/MI300X/MI325X `gfx942`, MI350P/MI350X/MI355X `gfx950`), Radeon (RX 7000 `gfx110x`, RX 9000
`gfx120x`, and others), and Ryzen APUs.
([compatibility matrix](https://rocm.docs.amd.com/en/latest/compatibility/compatibility-matrix.html))

**[I]** So gruflo's hardware span crosses three very different telemetry regimes, and the split matters more than the
model names:

| Regime | Examples | Telemetry consequences |
|---|---|---|
| CDNA 3/4 datacenter | MI300A/X, MI325X, MI350/355 | `gpu_metrics` v1.6+; partitions (XCP/XCC/AID); violations API; no `throttle_status`; per-partition metric scoping |
| CDNA 1/2 datacenter | MI100, MI210/250 | `gpu_metrics` v1.3; `throttle_status` + `indep_throttle_status`; no violations API; no partitions |
| RDNA / APU consumer | RX 7000/9000, Ryzen APUs | `gpu_metrics` v1.3 / v2.x / v3.0; fans; no partitions; no HBM sensors; several sysfs nodes absent |

**[R]** gruflo should model capability by **observed source availability**, not by ASIC name lists. Name lists rot;
the N/A sentinels and `AMDSMI_STATUS_NOT_SUPPORTED` are the contract AMD actually maintains (§10).

---

## 2. The four candidate source layers

### 2.1 `amd-smi` CLI with `--json`

**[F]** The CLI "uses Ctypes to call the `amd_smi_lib` API", and AMD attaches this disclaimer verbatim:

> The AMD SMI CLI tool is provided as an example code to aid the development of telemetry tools. The Python or
> C++ library is recommended as a robust data source.

([CLI tool usage](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html))

**[F]** Subcommands: `version list static firmware bad-pages metric process event topology set reset monitor
xgmi partition ras` (plus `fabric`, registered only when the amdgpu driver is initialized). Every command takes
the modifiers `--json | --csv`, `--file FILE`, `--loglevel LEVEL`. Bare `amd-smi --json` emits the default
summary. (same page)

**[F]** Watch modifiers exist on `metric`, `process`, and `monitor`: `-w/--watch INTERVAL` (seconds),
`-W/--watch_time TIME`, `-i/--iterations N`. (same page)

**[F]** Environment variables are honoured per invocation: `AMDSMI_GPU_METRICS_CACHE_MS` and
`AMDSMI_ASIC_INFO_CACHE_MS`. (same page)

**[I]** Cost model: one process spawn + Python interpreter start + `ctypes` load of `libamd_smi.so` + full library
init per sample. This is the layer AMD least wants you to depend on, and the only one with a documented
"example code" caveat.

**[R]** Do **not** put the CLI on gruflo's sampling path. Use it (a) as a cross-check oracle in tests, and (b) as
an optional one-shot diagnostic gruflo can point users at. gruflo must not require `rocm-cli` at runtime
(issue #1) — the same reasoning applies with more force to `amd-smi`, which is Python.

### 2.2 The AMD SMI C library (`libamd_smi.so`)

**[F]** Object model: `amdsmi_init()` → `amdsmi_get_socket_handles()` → `amdsmi_get_processor_handles(socket)` →
`amdsmi_shut_down()`. A *socket* is one physical GPU package (an APU socket holds both GPU and CPU processors);
a *processor handle* is a logical GPU.
([C++ library usage](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cpp-lib.html))

**[F]** "A device handle may change after restarting the application, so it should not be considered a persistent
identifier across processes." (same page)

**[F]** The library `dlopen`s `libdrm_amdgpu` at init, resolves `drmGetVersion`/`drmGetDevice`/`drmCommandWrite`,
and opens `/dev/dri/renderD<N>` with `O_RDWR | O_CLOEXEC | O_NONBLOCK` for each device.
([`amd_smi_drm.cc`](https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/src/amd_smi/amd_smi_drm.cc))

**[F]** It carries an internal singleton metrics cache shared across instances/threads, tuned by
`AMDSMI_GPU_METRICS_CACHE_MS` and `AMDSMI_ASIC_INFO_CACHE_MS`. (C++ library usage page)

**[F] Documented default disagreement.** The CLI page says `AMDSMI_GPU_METRICS_CACHE_MS` "Default 100"; the C++
library page says "1 ms". The same C++ page then names the variables `AMD_GPU_METRICS_CACHE_MS` /
`AMD_ASIC_INFO_CACHE_MS` (no `SMI`) in its "Best practice" section, contradicting its own table.
([CLI page](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html),
[C++ page](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cpp-lib.html))
**[R]** Treat the cache duration as unknown-by-default and set it explicitly from gruflo; do not rely on either
documented default, and do not rely on either spelling without probing both.

**[F]** ABI is not stable across ROCm majors. ROCm 10.0.0 bumped the SONAME to `libamd_smi.so.27` (breaking),
widened five `amdsmi_gpu_metrics_t` accumulators from 32- to 64-bit (changing struct layout and field offsets),
renamed types, removed APIs, and prefixed public macros with `AMDSMI_`.
([CHANGELOG](https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/CHANGELOG.md))

### 2.3 Kernel `amdgpu` / `amdkfd` sysfs + hwmon (zero runtime dependency)

**[F]** hwmon exposes, per device: `temp[1-3]_input` (millidegrees C), `temp[1-3]_label`, `temp[1-3]_crit`,
`temp[1-3]_crit_hyst`, `temp[1-3]_emergency`; `in0_input`/`in1_input` (mV); `power1_average` and `power1_input`
(microwatts, "on APUs this includes the CPU"), `power1_cap`, `power1_cap_min`, `power1_cap_max`; `fan1_input`
(RPM), `fan1_min`, `fan1_max`, `pwm1`, `pwm1_enable`; `freq1_input` (gfx/compute clock in Hz), `freq2_input`
(memory clock in Hz, dGPU only).
([GPU Power/Thermal Controls and Monitoring](https://docs.kernel.org/gpu/amdgpu/thermal.html))

**[F]** Device sysfs exposes `gpu_busy_percent` and `mem_busy_percent` — "The SMU firmware computes a percentage
of load based on the aggregate activity level in the IP cores" — plus `vcn_busy_percent`, and `gpu_metrics`
("a snapshot of all sensors at the same time"). (same page,
[`amdgpu_pm.c`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/pm/amdgpu_pm.c))

**[F]** Also: `mem_info_vram_total`, `mem_info_vram_used`, `mem_info_vis_vram_total`, `mem_info_vis_vram_used`,
`mem_info_gtt_total`, `mem_info_gtt_used` (all bytes); `pcie_bw` (received/sent message counts plus max payload
size, "estimating how much data has been received and sent … in the last second"); `pcie_replay_count`
(NAKs generated + received); `unique_id` (GFX9+ only, absent on GFX8 and older); `serial_number`,
`product_name`, `product_number`, `fru_id`, `manufacturer` ("only available for certain server cards");
`board_info` (form factor `cem`/`oam`/`unknown`).
([Misc AMDGPU driver information](https://docs.kernel.org/gpu/amdgpu/driver-misc.html))

**[F]** RAS sysfs: `/sys/class/drm/card*/device/ras/<block>_err_count` (lines `ue: N` / `ce: N`),
`/sys/class/drm/card*/device/ras/features`, and `/sys/class/drm/card*/device/ras/gpu_vram_bad_pages`
(`gpu_pfn : page_size : flag` where flag ∈ `R` reserved / `P` pending / `F` unable to reserve).
([AMDGPU RAS Support](https://docs.kernel.org/gpu/amdgpu/ras.html))

**[F]** XGMI hive identity is readable at `/sys/class/drm/card*/device/xgmi_info/xgmi_hive_id`, and the literal
physical hop count at `/sys/class/drm/card*/device/xgmi_num_hops`.
([CLI page](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html))

**[F] `gpu_metrics` is a versioned binary blob, not text.** Its first four bytes are
`struct metrics_table_header { uint16_t structure_size; uint8_t format_revision; uint8_t content_revision; }`,
and the driver memcpys the active `gpu_metrics_v*` struct into the read buffer.
([`kgd_pp_interface.h`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/kgd_pp_interface.h),
[`amdgpu_pm.c`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/pm/amdgpu_pm.c))
**[I]** A Rust reader can therefore parse `gpu_metrics` directly with a version dispatch on
`(format_revision, content_revision)` and no ROCm dependency at all — that is exactly what AMD SMI does
internally, and exactly why version skew produces N/A (§9).

### 2.4 debugfs

**[F]** `debugfs` carries GFXOFF state (`/sys/kernel/debug/dri/<N>/amdgpu_gfxoff`, `_status`, `_count`,
`_residency`; the last two "only supported in vangogh") and RAS **error injection** plus `auto_reboot` and
`ras_eeprom_reset` under `/sys/kernel/debug/dri/<N>/ras/`.
([thermal](https://docs.kernel.org/gpu/amdgpu/thermal.html),
[RAS](https://docs.kernel.org/gpu/amdgpu/ras.html))

**[R]** gruflo should not touch debugfs. It is root-only on virtually every distro, its only unique read-only
signal (GFXOFF residency) is single-ASIC, and the same directory contains destructive write interfaces. A
strictly read-only tool has no business having that path in its code.

---

## 3. Per-metric source map

Legend: **Primary** = the source gruflo should read; **Fallback** = what to use when the primary is absent;
**Perm** = R (any user) / G (needs `render`/`video` group) / S (needs root).

### 3.1 Hero-view candidates

| Metric | Primary source | Fallback | Unit / semantics | Perm | Notes |
|---|---|---|---|---|---|
| GFX busy | `amdsmi_get_gpu_activity()` → `gfx_activity` | sysfs `gpu_busy_percent` | % 0–100, SMU-aggregated | R | N/A sentinel is `0x0000FFFF`, **not** `0xFFFFFFFF` **[F]** (`amdsmi.h`, `amdsmi_engine_usage_t`) |
| Memory-controller busy | `amdsmi_get_gpu_activity()` → `umc_activity` | sysfs `mem_busy_percent` | % | R | `mem_busy_percent` is marked unsupported on APUs other than `gfx942`-class, and on GC 9.0.1 **[F]** (`amdgpu_pm.c`) |
| Multimedia busy | `amdsmi_get_gpu_activity()` → `mm_activity`; per-AID `vcn_activity` / `xcp_stats.vcn_busy` in `gpu_metrics` | sysfs `vcn_busy_percent` | % | R | `vcn_busy_percent` is gated to an explicit GC-version allowlist (mostly APUs, RDNA3/RDNA4) **[F]** (`amdgpu_pm.c`) |
| VRAM used / total | `amdsmi_get_gpu_vram_usage()` | sysfs `mem_info_vram_used` / `_total` | API returns **MB as `uint32`**; sysfs returns **bytes** | R | Precision differs between the two paths **[F]** (`amdsmi_vram_usage_t`; kernel driver-misc) |
| GTT used / total | sysfs `mem_info_gtt_used` / `_total` | — | bytes | R | On APUs `amd-smi monitor` switches its memory column to GTT because the GTT pool is larger **[F]** (CLI page) |
| Power (now) | `amdsmi_get_power_info()` → `socket_power`; `current_socket_power` (MI300+), `average_socket_power` (Navi/MI200 and earlier) | hwmon `power1_input` / `power1_average` | W via API; µW via hwmon | R | "socket_power … can rarely spike above the socket power limit"; unsupported members are `UINT32_MAX` **[F]** (`amdsmi.h`) |
| Power cap | `amdsmi_get_power_cap_info()`; `amd-smi static --limit` | hwmon `power1_cap` / `_cap_min` / `_cap_max` | W / µW | R | |
| Temperature | `amdsmi_get_temp_metric(type, AMDSMI_TEMP_CURRENT)` | hwmon `temp[1-3]_input` | °C via API; **milli**°C via hwmon | R | Sensor types include `EDGE`, `HOTSPOT`(=`JUNCTION`), `VRAM`, `HBM_0..3`, `PLX`, plus large GPU-board/baseboard ranges **[F]** (`amdsmi_temperature_type_t`) |
| Thermal limits | `AMDSMI_TEMP_CRITICAL` / `_EMERGENCY` / `_SHUTDOWN`; `amd-smi static --limit` | hwmon `temp*_crit`, `temp*_emergency` | °C | R | `static --limit` reports SLOWDOWN_* and SHUTDOWN_* for edge/hotspot/VRAM **[F]** (CLI page) |
| GFX / MEM clock | `amdsmi_get_clock_info(clk_type)` | hwmon `freq1_input` / `freq2_input`; sysfs `pp_dpm_sclk`/`_mclk` | MHz via API; **Hz** via hwmon | R | The API "reports the **averages over 1s** in MHz" and also returns `min_clk`, `max_clk`, `clk_locked`, `clk_deep_sleep` **[F]** (`amdsmi.h`) |
| Fan | `amdsmi_get_gpu_fan_rpms()` / `_fan_speed()` | hwmon `fan1_input`, `pwm1` | RPM / 0–255 | R | Instinct OAM parts commonly report N/A (the MI300A sample output shows `Fan: N/A`) **[F]** (CLI page) |
| Throttle / violation | see §8 | — | — | R | Mutually exclusive by generation |

### 3.2 Secondary-detail candidates

| Metric | Source | Notes |
|---|---|---|
| Energy | `amdsmi_get_energy_count()` → accumulator + `counter_resolution` (µJ) + timestamp (1 ns) **[F]**; `gpu_metrics.energy_accumulator` documented as "15.3 uJ resolution" **[F]** (py-api) | **[I]** Two energy readings + their timestamps give a clean average-power-over-window that is immune to the 100 ms sampling jitter of instantaneous power |
| PCIe link + errors | `amdsmi_get_pcie_info()` → `pcie_static{max_pcie_width, max_pcie_speed (MT/s), pcie_interface_version, slot_type}` and `pcie_metric{pcie_width, pcie_speed (MT/s), pcie_bandwidth (Mb/s), pcie_replay_count, pcie_l0_to_recovery_count, pcie_replay_roll_over_count, pcie_nak_sent_count, pcie_nak_received_count, pcie_lc_perf_other_end_recovery_count}` **[F]** (`amdsmi.h`) | sysfs fallbacks: `pcie_replay_count`, `pcie_bw`. `pcie_bw` is unsupported on APUs and where the ASIC lacks `get_pcie_usage` **[F]** (`amdgpu_pm.c`) |
| Coarse utilization counters | `amdsmi_get_utilization_count()` — "Every milliseconds the firmware calculates % busy count and then accumulates that value in the counter. This provides **minimally invasive** coarse grain GPU usage information." Types: coarse/fine GFX, MEM, DECODER **[F]** (`amdsmi.h`) | **[I]** The cheapest honest basis for a rolling-window busy figure: two counter reads + the returned 1 ns timestamps, no dependence on when the SMU last refreshed a gauge |
| Perf level / determinism | `amdsmi_get_gpu_perf_level()`; sysfs `power_dpm_force_performance_level` (`auto`/`low`/`high`/`manual`/`profile_*`) **[F]** (kernel thermal) | Read-only for gruflo |
| Voltages | `amdsmi_get_power_info()` → `gfx_voltage`/`soc_voltage`/`mem_voltage` (mV); hwmon `in0_input`/`in1_input` **[F]** | |
| XGMI | `amdsmi_get_xgmi_info()` (`xgmi_lanes`, `xgmi_hive_id`, `xgmi_node_id`), `amdsmi_get_gpu_xgmi_link_status()`, `gpu_metrics.xgmi_{link_width,link_speed,read_data_acc,write_data_acc}` **[F]** | Multi-GPU only |
| Board identity | `amdsmi_get_gpu_board_info()` (`model_number`, `product_serial`, `fru_id`, `product_name`, `manufacturer_name`), `amdsmi_get_gpu_asic_info()` (`market_name`, `device_id`, `asic_serial`, `num_compute_units`, `target_graphics_version`, `oam_id`) **[F]** | FRU fields are "only available for certain server cards" **[F]** (kernel driver-misc) |
| Enumeration identity | `amdsmi_get_gpu_enumeration_info()` → `drm_render`, `drm_card`, `hsa_id`, `hip_id`, `hip_uuid`, `oam_id` (`0xFFFFFFFF` if N/A) **[F]** | The stable cross-restart identity; see §7 |

---

## 4. Permissions

**[F]** All `amdgpu` device sysfs metric attributes are created read-only-to-everyone: `AMDGPU_DEVICE_ATTR_RO`
expands to `__ATTR(_name, S_IRUGO, …)`, and `gpu_busy_percent`, `mem_busy_percent`, `vcn_busy_percent`,
`pcie_bw`, and `gpu_metrics` are all declared with it.
([`amdgpu_pm.h`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/pm/inc/amdgpu_pm.h),
[`amdgpu_pm.c`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/pm/amdgpu_pm.c))

**[F]** hwmon sensors are likewise `S_IRUGO` (`SENSOR_DEVICE_ATTR(temp1_input, S_IRUGO, …)`,
`power1_average`, `power1_input`, …). (`amdgpu_pm.c`)

**[F]** RAS error counters are `S_IRUGO`: the per-block `<name>_err_count` attribute is built with
`.mode = S_IRUGO`, and `features`/`version`/`schema`/`event_state` are `S_IRUGO`/`0444`.
([`amdgpu_ras.c`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_ras.c))

**[F]** KFD per-process sysfs files use `KFD_SYSFS_FILE_MODE`, defined as `0444`.
([`kfd_priv.h`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_priv.h),
[`kfd_process.c`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_process.c))

**[F]** Device-node access is group-gated. ROCm's own install instructions say GPU access "is controlled by
membership in the `video` and `render` Linux system groups" (`sudo usermod -a -G render,video $LOGNAME`), with an
alternative udev rule setting `kfd` and `renderD*` to mode `0666`.
([Install AMD ROCm](https://rocm.docs.amd.com/en/latest/install/rocm.html))

**[F]** Process **names** need elevation: "Process Name may require elevated permissions. If running without
`sudo`, process names may appear as `N/A`."
([CLI page](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html))
The mechanism is `readlink("/proc/<pid>/exe")`, falling back to the literal string `"N/A"`, and failure to
`opendir("/proc/<pid>/fdinfo/")` returns `AMDSMI_STATUS_NO_PERM`.
([`fdinfo.cc`](https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/src/amd_smi/fdinfo.cc))

**[F]** APIs explicitly annotated "requires admin/sudo privileges" in `amdsmi.h` and relevant to a read-only
tool: `amdsmi_get_gpu_bad_page_threshold()`, `amdsmi_gpu_validate_ras_eeprom()`, and the partition-profile
config query family. `amd-smi static --ras` warns "Sudo may be required for some features", `amd-smi set` and
`amd-smi reset` state "Requires 'sudo' privileges", and every `amd-smi ras --cper` example is run under `sudo`.
([`amdsmi.h`](https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/include/amd_smi/amdsmi.h),
[CLI page](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html))

**Resulting permission tiers [I]:**

| Tier | What gruflo gets |
|---|---|
| **No group, no root** | Everything in `/sys/class/drm/*/device/` and its `hwmon/`: busy percentages, memory pools, temperatures, power, clocks, fans, PCIe counters, RAS error counts, bad-page list, board identity, raw `gpu_metrics`. Enough for a complete hero view. |
| **`render`/`video` group** | Additionally: the AMD SMI library itself (it opens `/dev/dri/renderD*` `O_RDWR`), KFD-based process→GPU association, KFD event notifications. |
| **Root** | Additionally: other users' process names and fdinfo, CPER/AFID records, bad-page threshold, EEPROM validation. |

**[R]** gruflo should degrade in exactly these tiers and *say which tier it is running in*, rather than showing
holes. A one-line footer such as `limited: not in 'render' group — process attribution unavailable` respects the
one-second-readability rule while being honest. Corollary: gruflo must be useful with **zero** privileges, which
argues for sysfs as the primary path and the library as an enrichment path.

---

## 5. Sampling cost and cadence constraints

**[F] The driver metric cache refreshes at 100 ms, and that is the floor.** "Violations are sampled every 100ms
— the fastest rate the driver can update the metric cache. Set `AMDSMI_GPU_METRICS_CACHE_MS=0` to disable AMD
SMI's internal cache and let the driver control when the cache updates."
([GPU violations](https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/gpu-violations.html))

**[F] Reads do not wake a sleeping GPU, and fail while it sleeps.** Every sysfs sensor read and the `gpu_metrics`
read go through `amdgpu_pm_get_access_if_active()`, which calls `pm_runtime_get_if_active()` and returns
`-EPERM` when the device is runtime-suspended: "Ignore runpm status. If device is in suspended state, deny
access." (`amdgpu_pm.c`)
**[I]** This is good news and a design constraint: polling an idle laptop dGPU costs nothing and will not
prevent it from sleeping, but gruflo will see `EPERM` and must render that as *asleep*, not as *error*.

**[F] Expensive calls, with their documented reasons:**

| Call | Documented cost |
|---|---|
| `amdsmi_get_violation_status()` | "**API will be slow due to polling driver for 2 samples. Require a minimum wait of 100ms between the 2 samples** in order to calculate. Otherwise users would need to use `amdsmi_get_gpu_metrics_info` for BM." (`amdsmi.h`) |
| `amdsmi_get_gpu_process_list()` | "**IMPORTANT: To get valid return values, at least 1 second needs to pass** from starting the program to the first call of this function, and before every following call of this function after that, to get correct values" (`amdsmi.h`) |
| `amdsmi_get_clock_info()` | "reports the **averages over 1s** in MHz" (`amdsmi.h`) |
| KFD `cu_occupancy` read | Each read allocates an `AMDGPU_MAX_QUEUES`-sized array and asks the hardware for live wave counts across the device's queues, then converts waves→CUs. It short-circuits to `0` when the process has no active queues. (`kfd_process.c`) |
| Per-process fdinfo scan | For each PID, AMD SMI `opendir`s `/proc/<pid>/fdinfo/` and reads **every** fd entry, re-opening and re-parsing any file whose `drm-pdev` matches the target BDF. (`fdinfo.cc`) |

**[F]** `amdsmi_get_utilization_count()` is described as "minimally invasive": the firmware accumulates a %-busy
value every millisecond and the API returns the accumulator plus a 1 ns-resolution timestamp. (`amdsmi.h`)

**[F]** `gpu_metrics` is explicitly "a snapshot of all sensors at the same time".
([kernel thermal](https://docs.kernel.org/gpu/amdgpu/thermal.html))

**[I] Cadence budget implied by the above:**

- Anything faster than ~10 Hz buys nothing: the underlying cache moves at 100 ms.
- A single `gpu_metrics` read yields temperature, activity, power, clocks, throttle state, fan, and accumulators
  **coherently**, versus N separate sysfs reads that are individually cheap but mutually skewed.
- Violation *percentages*, process lists, and clock *averages* are ≥1 s-scale quantities. Rendering them on a
  100 ms tick would be fabricating precision.

**[R]** Three cadences, not one:
1. **Fast (≈100–250 ms)** — one coherent `gpu_metrics`-class snapshot per device; drives the hero gauges and the
   breathing motion.
2. **Slow (≈1 s)** — violations/throttle percentages, clock averages, ECC counters, PCIe counters.
3. **Overlay-only (≈1–2 s, and only while the process overlay is visible)** — the process list, because it costs
   a full `/proc` fdinfo walk plus, for `cu_occupancy`, a live hardware query per process.

**[R]** Derive rate-like values from **accumulators and their timestamps** (utilization counters, energy
accumulator, `gfx_activity_acc`, `mem_activity_acc`) rather than from sampling gauges on a UI tick. That makes
gruflo's numbers correct under a slow or stalled render loop, which is the whole point of separating collection
cadence from display cadence (issue #3).

---

## 6. Schema and version stability

**[F] `gpu_metrics` is versioned and skew-sensitive.** AMD documents this failure mode plainly:

> The `amdgpu` driver reports a newer `gpu_metrics` version than the installed AMD SMI supports. `gpu_metrics`
> is a versioned structure supplied by the driver, and AMD SMI needs explicit support for each version's layout.
> When the driver is newer than your AMD SMI (or ROCm) release by a release cycle, AMD SMI can't parse the newer
> layout, so fields sourced from `gpu_metrics` (violations, `SOCKET_POWER`, engine usage, and so on) read N/A.

([CLI page, "About N/A values"](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html))
The install page states the symmetric case: an older driver than AMD SMI expects also produces N/A.
([install](https://rocm.docs.amd.com/projects/amdsmi/en/latest/install/install.html))

**[F]** Concrete versions in the kernel header today: `gpu_metrics_v1_0` … `v1_9`, `v2_0` … `v2_4`, `v3_0`.
v1.0 and v2.0 are marked "not recommended as it's not naturally aligned". **v1.9 abandons the fixed struct**
entirely for a self-describing list: `{ common_header; int attr_count; struct gpu_metrics_attr metrics_attrs[]; }`.
([`kgd_pp_interface.h`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/kgd_pp_interface.h))
AMD SMI adds: "ROCm 7.13 added support for the dynamic `gpu_metrics` layout introduced in v1.9, which handles
current and future versions, so releases from 7.13 onward are no longer affected by this mismatch."
([violations page](https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/gpu-violations.html))

**[F]** `amdsmi_get_gpu_metrics_header_info()` returns the header (i.e. the version) on its own, without parsing
the payload. (`amdsmi.h`)

**[F] The CLI's JSON schema has changed repeatedly.** From the CHANGELOG:
- `COMPUTE_PARTITION` renamed to `ACCELERATOR_PARTITION`.
- Scalar values were replaced by `{"value": N, "unit": "W"}` objects across `metric`, `static`, and `monitor`.
- JSON/CSV key casing normalized (`AID_<N>`→`aid_<N>`, `XCP_<N>`→`xcp_<N>`, `GPU<N>`→`gpu_<N>`).
- `lc_perf_other_end_recovery` renamed to `lc_perf_other_end_recovery_count`.
- Backwards compatibility for `jpeg_activity`/`vcn_activity` was **removed** in favour of
  `xcp_stats.jpeg_busy`/`xcp_stats.vcn_busy`.
- Multiple releases shipped *invalid* JSON: `amd-smi partition --json` emitted trailing non-JSON text;
  `amd-smi ras --cper --json` emitted one array per GPU, and emitted nothing at all when there were no entries,
  so `json.loads` failed.
([CHANGELOG](https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/CHANGELOG.md))

**[F]** The CHANGELOG's own preamble: "***All information listed below is for reference and subject to change.***"

**[I]** Stability ranking, most to least stable, for a tool that must keep working across ROCm releases:
1. **Kernel sysfs text nodes** (`gpu_busy_percent`, `mem_info_*`, hwmon, `ras/*_err_count`) — plain scalars
   governed by kernel ABI-stability norms; the oldest and least-churned surface here.
2. **`gpu_metrics` binary** — explicitly versioned with a self-describing header, so skew is *detectable*
   rather than silent.
3. **AMD SMI C API** — stable within a ROCm major, breaking across (SONAME 26→27).
4. **`amd-smi --json`** — no compatibility guarantee, documented as example code, with a history of renames,
   re-shapes, and outright malformed output.

**[R]** gruflo's collector should read sysfs + `gpu_metrics` directly and treat AMD SMI as an optional
enrichment backend. That satisfies "must not require rocm-cli at runtime", keeps gruflo installable on a host
with only the `amdgpu` driver, and removes the CLI's JSON churn from gruflo's blast radius.
**[R]** If a `gpu_metrics` version is unrecognised, report *"driver metrics v1.N unsupported by this gruflo build"*
rather than a wall of N/A. AMD's own N/A is the failure mode users already find confusing; gruflo can do better
by naming the cause.

---

## 7. Process attribution — what can be said honestly

**[F] There are two independent accounting systems, and they do not overlap.**

*DRM fdinfo* is written by `amdgpu_show_fdinfo()`, which reports the process's `amdgpu_vm` memory statistics and
per-IP times taken from `amdgpu_ctx_mgr_usage()` — i.e. **command submissions through the DRM/libdrm path**. Its
keys are `pasid`, the standard `drm-total-/shared-/resident-/purgeable-/active-<region>` set for regions
`vram|gtt|cpu|gds|gws|oa|doorbell|mmioremap`, the legacy aliases `drm-memory-vram|gtt|cpu` (KiB), the
amdgpu-specific `amd-evicted-vram`, `amd-requested-vram`, `amd-requested-gtt` (KiB), and
`drm-engine-<gfx|compute|dma|dec|enc|enc_1|jpeg|vpe>` in ns.
([`amdgpu_fdinfo.c`](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_fdinfo.c))

*KFD* accounts HSA/ROCm compute separately, under `/sys/class/kfd/kfd/proc/<pid>/`: `vram_<gpuid>`,
`sdma_<gpuid>`, `stats_<gpuid>/evicted_ms`, `stats_<gpuid>/cu_occupancy`, and
`counters_<gpuid>/{faults,page_in,page_out}` for SVM-capable devices. (`kfd_process.c`)

**[F] AMD SMI's per-process numbers come from fdinfo; KFD is used only to decide *membership*.**
`gpuvsmi_get_pid_info()` fills `amdsmi_proc_info_t` from `drm-memory-gtt`, `drm-memory-cpu`, `drm-memory-vram`,
`drm-engine-gfx`, and `drm-engine-enc` in `/proc/<pid>/fdinfo/*` whose `drm-pdev` matches the device BDF.
`gpu_is_in_kfd_pid()` is consulted first purely to test whether the PID uses that GPU, with a BDF-matching
fallback. (`fdinfo.cc`)

**[F] The kernel deprecates the very keys AMD SMI reads:** "`drm-memory-<region>` … This key is deprecated and is
only printed by amdgpu; it is an alias for `drm-resident-<region>`."
([DRM client usage stats](https://docs.kernel.org/gpu/drm-usage-stats.html))

**[F] AMD SMI's KiB→bytes conversion is decimal.** The kernel prints these values in **KiB**
(`stats[TTM_PL_VRAM].drm.resident/1024UL` with a literal `" KiB"` suffix), while `fdinfo.cc` accumulates
`info.mem += mem * 1000` and `info.memory_usage.vram_mem += mem * 1000`, with the struct field documented as
"In Bytes". (`amdgpu_fdinfo.c`, `fdinfo.cc`, `amdsmi.h`)
**[I]** Per-process memory reported by AMD SMI is therefore about **2.4 % lower** than the true byte count.
**[R]** If gruflo reads fdinfo itself it should use `drm-resident-<region>` (the non-deprecated key) and multiply
by 1024. If gruflo instead consumes AMD SMI's numbers, it must not present them as exact bytes.

**[F] Aggregation is explicitly not conservative:** "Sum of the process memory is not expected to be the total
memory usage." (`amdsmi.h`, on `amdsmi_get_gpu_process_list` and `amdsmi_process_info_t`)

**[F] `cu_occupancy` is a CU *count*, and the sources disagree about that.** The kernel computes
`cu_cnt = (wave_cnt + (max_waves_per_cu - 1)) / max_waves_per_cu` and prints that integer (`kfd_process.c`).
`amdsmi_proc_info_t.cu_occupancy` is commented "Num CUs utilized"; the legacy `amdsmi_process_info_t.cu_occupancy`
is commented "Compute Unit usage in percent"; the Python reference says "Number of Compute Units utilized"; and
the CLI page describes the default table's `CU %` column as "compute unit occupancy percentage".
(`amdsmi.h`, [py-api](https://rocm.docs.amd.com/projects/amdsmi/en/latest/reference/amdsmi-py-api.html), CLI page)
**[R]** The kernel is authoritative: label it **CUs**, not %. If gruflo wants a percentage it must divide by the
device's `num_compute_units` from `amdsmi_get_gpu_asic_info()` and say so.

**[F]** Other honest per-process fields: `evicted_time` (ms queues were evicted, from `evicted_ms`),
`sdma_usage` (µs), `container_name` (parsed out of `/proc/<pid>/cgroup`), and the per-PID cross-GPU rollup
`amdsmi_get_gpu_process_list_by_pid()` returning `amdsmi_proc_info_by_pid_t` with a per-GPU breakdown sorted by
PID. (`amdsmi.h`, `fdinfo.cc`)

**[I] What gruflo can and cannot claim in the process overlay:**

| Claim | Honest? |
|---|---|
| "This PID has GPU N open" | **Yes** — KFD membership or fdinfo `drm-pdev` match |
| "This PID holds X bytes of VRAM/GTT/CPU memory" | **Yes, with caveats** — resident bytes only; the sum will not equal device usage; do not use AMD SMI's decimal-KiB value as exact |
| "This PID has spent T ns on the gfx/enc engine" | **Yes** — monotonic ns counters (the kernel warns they may briefly regress and that readers must hold the previous maximum until a monotonic update appears) |
| "This PID is using P % of the GPU" | **No.** No such value exists at any layer. Engine-time deltas cover only DRM command submissions and exclude HSA/KFD compute queues used by HIP |
| "This PID occupies C compute units" | **Yes, instantaneously** — a live wave-count sample, not an average, and only where the ASIC implements `get_cu_occupancy` |

**[R]** The process overlay must state, in the UI, that per-process figures are **memory and engine time**, not a
share of utilization, and that they cover DRM submissions. This is precisely the "where must the overlay state
that utilization cannot be attributed reliably" question in issue #4, and the answer is: everywhere a
percentage would otherwise be implied.

---

## 8. Partitioned devices

**[F] Vocabulary** (MI300-class): XCD = compute die; XCC = the logical compute core the driver exposes
(one per XCD on current parts); AID = active interposer die carrying PCIe, xGMI, and HBM controllers;
**XCP** = the logical GPU produced by a partition. MI300X has 8 XCDs across 4 AIDs; MI300A has 6.
([GPU partitioning](https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/partition.html))

**[F] Modes.** Accelerator: `SPX` (1 logical GPU), `DPX` (2), `TPX` (3), `QPX` (4), `CPX` (one per XCC → 8 on
MI300X). Memory: `NPS1`/`NPS2`/`NPS4`/`NPS8`. (same page)

**[F] Handle model.** A socket handle is one physical GPU package; `amdsmi_get_processor_handles(socket)` returns
one handle per XCP. Handles become invalid after a partition-mode change and must be re-enumerated. (same page)

**[F] Primary vs secondary partitions — the load-bearing fact for a monitoring tool:**

> Partition (XCP) and device-level metrics come from **separate sysfs sources**. The device-wide `gpu_metrics`
> node exists only on the **primary partition** (XCP 0, e.g. `renderD128/device/gpu_metrics`), so only the
> primary partition can report whole-GPU values such as board power. **Secondary partitions** expose only their
> own `xcp_metrics` (e.g. `renderD129/device/xcp/xcp_metrics`) …

(same page). The CLI page demonstrates the consequence on an MI300X in `CPX`/`NPS4`: rows for XCP 1–3 show
`N/A` for POWER, GPU_T, MEM_T, GFX_CLK, GFX%, MEM%, and DEC%, while still reporting their own VRAM usage —
"only the primary XCP of each partition reports per-engine sensors, while the other XCPs share the physical
device and report `N/A` for those fields".
([CLI page](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html))

**[F] Metric scoping.** Socket power (`socket_power`, `average_socket_power`) is a socket-level, whole-GPU value;
clocks, utilization, and violations are per-XCP. `amdsmi_get_gpu_partition_metrics_info()` retrieves the
partition-scoped table, and `amd-smi metric -X/--partition` switches temperature/clock/usage to
XCP/AID/MID-scoped sources ("Only available for MI300 or newer ASICs"). (partition page, CLI page)

**[F] Identity under partitioning.** All XCPs of one physical GPU share a Bus:Device address; the partition ID
lives in bits [31:28] of the internal 64-bit BDFID, with a documented fallback to bits [2:0] (the PCIe function
field) "when bits [31:28] are zero and bits [2:0] are non-zero (common in non-SPX modes on certain driver
versions)". From **ROCm 7.0** each XCP gets its own UUID; before that all partitions shared one. In
**ROCm 7.13.0** `amdsmi_get_gpu_device_uuid()` was realigned to the HIP/`rocminfo` format. From **ROCm 6.4.1**
each partition maps to its own DRM render minor (previously they all mirrored `renderD128`). (partition page)

**[F] Virtualization.** Inside an SR-IOV guest the reported partition mode "will **not** reflect the actual
accelerator partition mode configured on the host … the hypervisor withholds host partition details from guest
VMs for security reasons." (partition page)

**[I] Consequences for gruflo:**
- The count of "GPUs" gruflo shows is a partition-mode artifact. An 8-card MI300X host in CPX reports 64 logical
  GPUs. A dashboard that answers "what is my GPU doing" in one second must not present 64 equal tiles.
- Naïvely summing power across processor handles **double-counts by up to 8×**, because socket power is
  whole-package and repeated per XCP.
- Blank per-engine fields on secondary XCPs are *structural*, not a fault, and must be rendered differently from
  "sensor missing" and from "permission denied".

**[R]** Group by socket in the UI: one physical GPU per card, partitions as sub-rows. Attribute power, board
temperature, and fan to the socket; attribute clocks, activity, memory, and violations to the XCP. Label
secondary-partition blanks as *"reported by primary partition"*, not `N/A`. **[R]** Never aggregate a
socket-scoped metric across processor handles.

---

## 9. Fault, throttle, and health signals

### 9.1 Throttling — two mutually exclusive mechanisms

**[F]** MI300 Series and newer (`gpu_metrics` v1.6+) use `amdsmi_get_violation_status()` /
`amd-smi metric --violation` / `amd-smi monitor --violation`. **On these GPUs `throttle_status` in
`amd-smi metric --power` reports N/A.** Radeon (Navi) and MI100/MI200 (`gpu_metrics` v1.3) use
`throttle_status` and `indep_throttle_status` from `amdsmi_get_gpu_metrics_info()`; **on these GPUs the
violations API returns N/A or max_uint.** "The two mechanisms aren't interchangeable: the violations API measures
*how much* throttling occurred (PVIOL%, TVIOL%); `throttle_status` measures *whether* it's happening now."
([GPU violations](https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/gpu-violations.html))

**[F]** `amdsmi_violation_status_t` gives three parallel field families per violation type: `acc_*` (uint64 raw
accumulator), `active_*` (uint8 0/1 currently active), `per_*` (uint64 % of the sampling period in violation).
Core types: PROCHOT (`*_prochot_thrm`), PPT/power = **PVIOL** (`*_ppt_pwr`), socket thermal = **TVIOL**
(`*_socket_thrm`), VR thermal (`*_vr_thrm`), HBM thermal (`*_hbm_thrm`). With `gpu_metrics` v1.8+ there are
additional 2-D `[XCP][XCC]` arrays: `*_gfx_clk_below_host_limit_pwr|_thm|_total` and `*_low_utilization`.
Metadata: `reference_timestamp` (µs), `violation_timestamp` (ns on bare metal), `acc_counter`. Unsupported
fields read `max_uint64`/`max_uint8`. (same page)

**[F]** For Navi/MI1xx/MI2xx, `throttle_status` (uint32) says *whether*; `indep_throttle_status` (uint64) says
*why*, as raw bits AMD SMI "passes … through to the caller without interpreting". The canonical definitions are
the `SMU_THROTTLER_*` enum in the driver's `amdgpu_smu.h`; the documented ranges are bits 0–7 power
(PPT0, PPT1, SPL, FPPT, SPPT), 16–23 current (TDC_GFX, TDC_SOC, TDC_MEM, EDC_CPU, EDC_GFX), 32–47 temperature
(TEMP_GPU, TEMP_MEM, TEMP_HOTSPOT, TEMP_SOC, TEMP_VR_GFX, PROCHOT_GFX). (same page)

**[F]** AMD publishes an interpretation table for violation percentages: 0 % none, 1–25 % light, 25–50 %
moderate, 50–100 % heavy, N/A/max_uint unsupported. (same page)

### 9.2 ECC / RAS

**[F]** `amdsmi_get_gpu_total_ecc_count()`, `amdsmi_get_gpu_ecc_count()` (per block),
`amdsmi_get_gpu_ecc_enabled()`, `amdsmi_get_gpu_ecc_status()`, and `amdsmi_get_gpu_ras_block_features_enabled()`.
`amdsmi_error_count_t` = `{correctable_count, uncorrectable_count, deferred_count}`. `amd-smi metric -e` prints
TOTAL_CORRECTABLE / TOTAL_UNCORRECTABLE / TOTAL_DEFERRED / CACHE_CORRECTABLE / CACHE_UNCORRECTABLE; `-k` breaks
it down per IP block (UMC, SDMA, GFX, MMHUB, PCIE_BIF, HDP, XGMI_WAFL, …).
(`amdsmi.h`, [RAS](https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/ras.html))

**[F]** The kernel equivalent, world-readable and dependency-free:
`/sys/class/drm/card*/device/ras/<block>_err_count`, format `ue: N` / `ce: N`.
([kernel RAS](https://docs.kernel.org/gpu/amdgpu/ras.html))

**[F]** `amdsmi_get_gpu_ras_feature_info()` returns `{ras_eeprom_version, ecc_correction_schema_flag,
ras_info{dram_ecc, sram_ecc, poisoning}, needs_reboot}`. The `needs_reboot` flag is a first-class health signal.
(`amdsmi.h`)

**[F]** Bad pages: `amdsmi_get_gpu_bad_page_info()` and `amdsmi_get_gpu_memory_reserved_pages()`, with page
states `AMDSMI_MEM_PAGE_STATUS_{RESERVED, PENDING, UNRESERVABLE}`; `amd-smi bad-pages -p/-r/-u`. Kernel source:
`/sys/class/drm/card*/device/ras/gpu_vram_bad_pages`, flags `R`/`P`/`F`. `amd-smi static --ras` also reports
`BAD_PAGE_THRESHOLD` and `BAD_PAGE_THRESHOLD_EXCEEDED`. (`amdsmi.h`, kernel RAS, CLI page)

**[F]** CPER / AFID: `amdsmi_get_gpu_cper_entries()`, `amdsmi_get_afids_from_cper()`, `amd-smi ras --cper`
with severities `nonfatal-uncorrected | fatal | nonfatal-corrected | all` and `--follow`. CPER severity encoding
is 0 recoverable / 1 fatal / 2 corrected / 3 informational. All AMD examples run this under `sudo`, and it
writes record files to a folder. (RAS page, CLI page)
**[R]** Out of scope for gruflo: it is root-only, file-emitting, and fleet-oriented. Surface *that CPER records
exist* only if a zero-cost, non-root indicator turns up on hardware; otherwise point the user at `amd-smi ras`.

### 9.3 Push-based events

**[F]** `amdsmi_init_gpu_event_notification()` → `amdsmi_set_gpu_event_notification_mask(handle, mask)` →
`amdsmi_get_gpu_event_notification(timeout_ms, &num_elem, data)` → `amdsmi_stop_gpu_event_notification()`.
Event types: `VMFAULT`(1), `THERMAL_THROTTLE`(2), `GPU_PRE_RESET`(3), `GPU_POST_RESET`(4), `MIGRATE_START`(5),
`MIGRATE_END`(6), `PAGE_FAULT_START`(7), `PAGE_FAULT_END`(8), `QUEUE_EVICTION`(9), `QUEUE_RESTORE`(10),
`UNMAP_FROM_GPU`(11), `PROCESS_START`(12), `PROCESS_END`(13). Each delivered event carries a processor handle and
a message string. (`amdsmi.h`)
**[I]** This is the only *event*-shaped health source; everything else is a poll. It is a blocking call with a
timeout, so it wants its own thread.
**[R]** Strong candidate for gruflo's explainable health line: "thermal throttle 3 s ago", "VM fault in PID 1234",
"GPU reset" are all one-second-readable, causal statements — exactly the kind of signal issue #5 asks for
instead of an opaque health score. Defer to v2 if the extra thread is not justified, but do not replace it with a
score.

### 9.4 Other health-adjacent counters

**[F]** PCIe: `pcie_replay_count`, `pcie_l0_to_recovery_count`, `pcie_replay_roll_over_count`,
`pcie_nak_sent_count`, `pcie_nak_received_count` (`amdsmi_get_pcie_info()`), plus sysfs `pcie_replay_count`.
XGMI: `amdsmi_get_xgmi_info()`, `amdsmi_get_gpu_xgmi_link_status()`, `amd-smi metric --xgmi-err`
("XGMI error information since last read" — a **read-and-clear** style counter). Thermal limits:
`SLOWDOWN_*`/`SHUTDOWN_*` from `amd-smi static --limit`, or hwmon `temp*_crit` / `temp*_emergency`.
(`amdsmi.h`, CLI page, kernel thermal)

**[I] An explainable health line can be assembled entirely from source-reported signals** — no scoring
required: *is it throttling and why* (violations `active_*` / `indep_throttle_status` bits), *is it near a limit*
(hotspot vs `SLOWDOWN_HOTSPOT_TEMPERATURE`; power vs cap), *has memory degraded* (uncorrectable ECC ≠ 0,
pending/unreservable bad pages, `needs_reboot`), *is the link degraded* (replay/NAK deltas, XGMI errors), and
*has something faulted recently* (event notifications).

---

## 10. Capability and absence semantics

**[F] AMD's own taxonomy for `N/A`:** "**Not Applicable**" — the feature does not apply to this hardware or
configuration (display clocks on a headless compute card; partition details on an unpartitioned GPU); or
"**Not Available**" — the component does not report the metric, the installed driver cannot be queried for it
through `amd-smi-lib`, or the driver's `gpu_metrics` version is unsupported.
([CLI page, "About N/A values"](https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html))

**[F] Sentinels and status codes gruflo must recognise:**

| Signal | Meaning | Source |
|---|---|---|
| `gfx_activity == 0x0000FFFF` | activity unavailable — note it is a `uint16` max carried in a `uint32` field, **not** `0xFFFFFFFF` | `amdsmi_engine_usage_t` |
| `amdsmi_power_info_t` member `== UINT32_MAX` | that member unsupported | `amdsmi.h` |
| violation field `== max_uint64` / `max_uint8` | that violation type unsupported on this ASIC | violations page |
| `oam_id == 0xFFFFFFFF` | no OAM id | `amdsmi_enumeration_info_t` |
| `AMDSMI_STATUS_NOT_SUPPORTED` | feature absent | `amdsmi_status_t` |
| `AMDSMI_STATUS_NO_PERM` (10) | permission denied | `amdsmi_status_t` |
| `AMDSMI_STATUS_UNEXPECTED_DATA` | sysfs content unparseable or empty (e.g. `amdsmi_get_vcn_busy_percent`) | `amdsmi.h` |
| sysfs read → `-EPERM` | device runtime-suspended | `amdgpu_pm.c` |
| sysfs node absent | ASIC/driver does not implement it (e.g. `unique_id` pre-GFX9, `uma/` on non-APUs) | kernel driver-misc, CLI page |

**[R]** gruflo needs at least five distinguishable absence states, and they should look different on screen:
**unsupported by hardware**, **unsupported by this driver/metrics version**, **permission denied**,
**asleep (runtime-suspended)**, and **reported by the primary partition**. Collapsing all five into `N/A` is the
exact confusion AMD needed a documentation section to explain — gruflo can simply not have that problem.
**[R]** Model capability once at startup by probing, then cache it; do not re-probe unavailable metrics on every
tick.

---

## 11. Recommendations for gruflo (consolidated)

All **[R]**; these are inputs to the decisions in issues #3, #4, #5, and #9, not decisions.

1. **Two backends behind one trait.** `SysfsBackend` (mandatory, zero-dependency, zero-privilege: kernel sysfs +
   hwmon + `gpu_metrics` + KFD proc stats) and `AmdSmiBackend` (optional, `dlopen("libamd_smi.so.N")` at runtime,
   never a link-time dependency). gruflo must run and be useful with only the `amdgpu` driver present.
2. **`dlopen`, never link.** The SONAME moved 26→27 within one ROCm release; a hard link makes gruflo
   uninstallable on the wrong ROCm. Probe with `amdsmi_get_lib_version()` and refuse unknown majors gracefully.
3. **Never spawn `amd-smi`** on the sampling path. Keep it as a test oracle and a diagnostic suggestion.
4. **Prefer one coherent `gpu_metrics` snapshot** over N independent sysfs reads for the hero view; the kernel
   documents it as a same-instant snapshot of all sensors.
5. **Three cadences** (§5): ~100–250 ms hero snapshot, ~1 s slow metrics, ~1–2 s process overlay while visible.
   Never sample faster than the 100 ms driver cache.
6. **Derive rates from accumulators + timestamps**, not from differencing gauges on a UI tick.
7. **Group by socket, scope by XCP** (§8). Never sum socket-scoped values across processor handles.
8. **Five distinct absence states** (§10), never a bare `N/A`.
9. **Process overlay states its own limits** (§7): memory and engine time only; no per-process utilization
   percentage exists; `cu_occupancy` is CUs, not percent; the per-process sum is not the device total.
10. **Health is a sentence, not a score** (§9.4): name the throttle reason, the limit approached, or the fault
    observed, sourced from `active_*` / `indep_throttle_status` / ECC / event notifications.
11. **Announce the privilege tier** (§4) in one line rather than silently hiding the process overlay.
12. **Pin the identity key.** Use `amdsmi_get_gpu_enumeration_info()` (`hip_uuid`, `drm_render`, `oam_id`) or the
    sysfs `unique_id`/BDF for persisted daily summaries. Processor handles are explicitly not stable across
    process restarts, and UUID semantics changed in both ROCm 7.0 and 7.13.

---

## 12. Open questions requiring hardware [?]

1. **Real cost per sample.** Wall-clock for: one `gpu_metrics` read; the equivalent N individual sysfs reads;
   `amdsmi_get_gpu_activity()`; a full `amdsmi_get_gpu_process_list()`; and one `cu_occupancy` read against a
   process with many active queues. Needed to size the tick budget in issue #3.
2. **`AMDSMI_GPU_METRICS_CACHE_MS` reality.** Which spelling is honoured (`AMDSMI_*` vs `AMD_*`), and what the
   real default is — AMD's two pages disagree (100 ms vs 1 ms).
3. **Does polling perturb anything?** Whether a 10 Hz `gpu_metrics` poll measurably affects a running workload,
   and whether it interacts with GFXOFF residency on APUs/RDNA.
4. **`gpu_metrics` version coverage in the wild.** Which `(format_revision, content_revision)` pairs appear on
   MI300X, MI210, RX 7900, RX 9070, and a Ryzen APU — and whether v1.9's dynamic layout is what current drivers
   actually emit.
5. **Secondary-partition sysfs shape.** Confirm `renderD<N>/device/xcp/xcp_metrics` exists and what it contains,
   and whether `gpu_busy_percent` is present on secondary XCP nodes at all (the CLI shows GFX% as `N/A` there).
6. **KFD proc tree readability as a plain user.** Files are `0444`, but the directory modes and whether another
   user's `stats_<gpuid>` is traversable were not verified.
7. **`cu_occupancy` availability.** Which ASICs implement `kfd2kgd->get_cu_occupancy` (the file is only created
   when they do) — likely narrower than the amd-smi hardware span.
8. **fdinfo vs KFD coverage in practice.** For a running HIP/PyTorch process, whether `drm-engine-gfx` and
   `drm-engine-compute` advance at all, or whether all HSA queue work is invisible to fdinfo. This determines
   whether the process overlay can show *any* engine time for ROCm compute workloads, and is the single most
   important unknown for issue #4.
9. **`amd-smi --json` shape on a current release.** If gruflo ever ships a compatibility mode, capture real JSON
   from at least two ROCm versions to size the churn.
10. **APU behaviour.** On a Ryzen APU: whether `power1_average` includes the CPU (the kernel says it does),
    whether `is_apu`/`apu_metrics.*` populate, and what the VRAM/GTT split looks like when
    `MEM_CARVEOUT` applies.

---

## 13. Sources

**AMD SMI documentation** (ROCm docs, AMD SMI 26.5.0 / 27.0.0 / ROCm 7.14.0):
- CLI tool usage — <https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cli-tool.html>
- C++ library usage — <https://rocm.docs.amd.com/projects/amdsmi/en/latest/how-to/amdsmi-cpp-lib.html>
- Python API reference — <https://rocm.docs.amd.com/projects/amdsmi/en/latest/reference/amdsmi-py-api.html>
- GPU violations — <https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/gpu-violations.html>
- GPU partitioning — <https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/partition.html>
- Reliability, availability, serviceability — <https://rocm.docs.amd.com/projects/amdsmi/en/latest/conceptual/ras.html>
- Install the AMD SMI library and CLI tool — <https://rocm.docs.amd.com/projects/amdsmi/en/latest/install/install.html>

**AMD SMI source** (`ROCm/rocm-systems`, branch `develop`):
- `include/amd_smi/amdsmi.h` — <https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/include/amd_smi/amdsmi.h>
- `src/amd_smi/fdinfo.cc` — <https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/src/amd_smi/fdinfo.cc>
- `src/amd_smi/amd_smi_drm.cc` — <https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/src/amd_smi/amd_smi_drm.cc>
- `CHANGELOG.md` — <https://github.com/ROCm/rocm-systems/blob/develop/projects/amdsmi/CHANGELOG.md>
- Deprecation notice on the old repo — <https://github.com/ROCm/amdsmi>

**ROCm platform documentation:**
- Install AMD ROCm (GPU access permissions: `render`/`video` groups, udev rules) — <https://rocm.docs.amd.com/en/latest/install/rocm.html>
- ROCm compatibility matrix (supported GPU list) — <https://rocm.docs.amd.com/en/latest/compatibility/compatibility-matrix.html>

**Linux kernel documentation:**
- GPU Power/Thermal Controls and Monitoring (hwmon, `gpu_metrics`, `*_busy_percent`, GFXOFF debugfs) — <https://docs.kernel.org/gpu/amdgpu/thermal.html>
- Misc AMDGPU driver information (product/FRU, memory pools, `pcie_bw`, `pcie_replay_count`, `unique_id`, UMA carveout) — <https://docs.kernel.org/gpu/amdgpu/driver-misc.html>
- AMDGPU RAS Support (error-count sysfs, bad pages, injection debugfs) — <https://docs.kernel.org/gpu/amdgpu/ras.html>
- DRM client usage stats (fdinfo key specification) — <https://docs.kernel.org/gpu/drm-usage-stats.html>

**Linux kernel source** (`torvalds/linux`, branch `master`):
- `drivers/gpu/drm/amd/amdgpu/amdgpu_fdinfo.c` — <https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_fdinfo.c>
- `drivers/gpu/drm/amd/amdkfd/kfd_process.c` — <https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_process.c>
- `drivers/gpu/drm/amd/amdkfd/kfd_priv.h` — <https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdkfd/kfd_priv.h>
- `drivers/gpu/drm/amd/pm/amdgpu_pm.c` — <https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/pm/amdgpu_pm.c>
- `drivers/gpu/drm/amd/pm/inc/amdgpu_pm.h` — <https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/pm/inc/amdgpu_pm.h>
- `drivers/gpu/drm/amd/amdgpu/amdgpu_ras.c` — <https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/amdgpu/amdgpu_ras.c>
- `drivers/gpu/drm/amd/include/kgd_pp_interface.h` (`gpu_metrics_v*`, `metrics_table_header`) — <https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/kgd_pp_interface.h>
- `drivers/gpu/drm/amd/pm/swsmu/inc/amdgpu_smu.h` (`SMU_THROTTLER_*` bit definitions, cited by AMD as canonical) — <https://github.com/ROCm/amdgpu/blob/master/drivers/gpu/drm/amd/pm/swsmu/inc/amdgpu_smu.h>

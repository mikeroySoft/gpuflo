#!/usr/bin/env bash
set -Eeuo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly RESULTS_ROOT="$SCRIPT_DIR/results"
readonly RUN_STAMP="$(date -u +%Y%m%dT%H%M%S%NZ)"
readonly TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/gruflo-process-attribution.XXXXXX")"
readonly STAGING="$TMP_ROOT/$RUN_STAMP"

WORKLOAD_PID=""
WORKLOAD_KIND=""
FINAL_RESULT=""
FINAL_STATUS=0

cleanup() {
    local status=$?
    trap - EXIT INT TERM HUP
    if [[ -n "$WORKLOAD_PID" ]] && kill -0 "$WORKLOAD_PID" 2>/dev/null; then
        kill "$WORKLOAD_PID" 2>/dev/null || true
        wait "$WORKLOAD_PID" 2>/dev/null || true
    fi
    if ((status != 0)) && [[ -d "$STAGING" && -z "$FINAL_RESULT" ]]; then
        printf 'gruflo process-attribution capture: partial evidence was discarded after failure\n' >&2
    fi
    rm -rf -- "$TMP_ROOT"
    exit "$status"
}
trap cleanup EXIT INT TERM HUP

fail() {
    printf 'gruflo process-attribution capture: %s\n' "$*" >&2
    exit 1
}

[[ "$(uname -s)" == "Linux" ]] || fail "Linux is required"

shopt -s nullglob
render_nodes=(/sys/class/drm/renderD*)
amdgpu_nodes=()
for node in "${render_nodes[@]}"; do
    driver="$(readlink -f "$node/device/driver" 2>/dev/null || true)"
    if [[ "${driver##*/}" == "amdgpu" ]]; then
        amdgpu_nodes+=("$node")
    fi
done
((${#amdgpu_nodes[@]} > 0)) || fail "no AMD GPU bound to amdgpu was found"

umask 077
mkdir -p "$STAGING"

write_pytorch_workload() {
    cat > "$TMP_ROOT/workload.py" <<'PY'
import pathlib
import sys
import time

import torch

if not torch.version.hip or not torch.cuda.is_available():
    raise SystemExit("ROCm-enabled PyTorch with an available GPU is required")

duration = float(sys.argv[1])
ready = pathlib.Path(sys.argv[2])
device = torch.device("cuda:0")
size = 2048
a = torch.randn((size, size), device=device, dtype=torch.float32)
b = torch.randn((size, size), device=device, dtype=torch.float32)
for _ in range(3):
    torch.mm(a, b)
torch.cuda.synchronize()
ready.touch()
start = time.perf_counter()
iterations = 0
while time.perf_counter() - start < duration:
    torch.mm(a, b)
    iterations += 1
torch.cuda.synchronize()
elapsed = time.perf_counter() - start
print("workload=pytorch")
print(f"elapsed_seconds={elapsed:.6f}")
print(f"iterations={iterations}")
print(f"iterations_per_second={iterations / elapsed:.6f}")
PY
}

write_hip_workload() {
    cat > "$TMP_ROOT/workload.cpp" <<'CPP'
#include <hip/hip_runtime.h>

#include <chrono>
#include <cstddef>
#include <cstdint>

#include <cstdlib>
#include <fstream>
#include <iostream>

#define HIP_CHECK(call)                                                        \
    do {                                                                       \
        hipError_t status = (call);                                             \
        if (status != hipSuccess) {                                             \
            std::cerr << hipGetErrorString(status) << '\n';                    \
            return 1;                                                          \
        }                                                                      \
    } while (0)

__global__ void saxpy(const float* a, const float* b, float* c, std::size_t n) {
    std::size_t i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < n) c[i] = 1.25f * a[i] + b[i];
}

int main(int argc, char** argv) {
    if (argc != 3) return 2;
    const double duration = std::strtod(argv[1], nullptr);
    const std::size_t n = 16u * 1024u * 1024u;
    const std::size_t bytes = n * sizeof(float);
    float *a = nullptr, *b = nullptr, *c = nullptr;
    HIP_CHECK(hipMalloc(&a, bytes));
    HIP_CHECK(hipMalloc(&b, bytes));
    HIP_CHECK(hipMalloc(&c, bytes));
    HIP_CHECK(hipMemset(a, 1, bytes));
    HIP_CHECK(hipMemset(b, 2, bytes));
    const dim3 block(256);
    const dim3 grid((n + block.x - 1) / block.x);
    for (int i = 0; i < 3; ++i) hipLaunchKernelGGL(saxpy, grid, block, 0, 0, a, b, c, n);
    HIP_CHECK(hipDeviceSynchronize());
    std::ofstream(argv[2]).close();
    const auto start = std::chrono::steady_clock::now();
    std::uint64_t iterations = 0;
    double elapsed = 0.0;
    do {
        hipLaunchKernelGGL(saxpy, grid, block, 0, 0, a, b, c, n);
        HIP_CHECK(hipDeviceSynchronize());
        ++iterations;
        elapsed = std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count();
    } while (elapsed < duration);
    std::cout << "workload=hip\n"
              << "elapsed_seconds=" << elapsed << '\n'
              << "iterations=" << iterations << '\n'
              << "iterations_per_second=" << (iterations / elapsed) << '\n';
    hipFree(c);
    hipFree(b);
    hipFree(a);
    return 0;
}
CPP
}

select_workload() {
    if command -v python3 >/dev/null 2>&1 &&
        python3 -c 'import torch, sys; sys.exit(0 if torch.version.hip and torch.cuda.is_available() else 1)' \
            >/dev/null 2>&1; then
        write_pytorch_workload
        WORKLOAD_KIND="pytorch"
        WORKLOAD_CMD=(python3 "$TMP_ROOT/workload.py")
        return
    fi

    if command -v hipcc >/dev/null 2>&1; then
        write_hip_workload
        if hipcc -O2 "$TMP_ROOT/workload.cpp" -o "$TMP_ROOT/workload" >"$TMP_ROOT/hipcc.log" 2>&1; then
            WORKLOAD_KIND="hip"
            WORKLOAD_CMD=("$TMP_ROOT/workload")
            return
        fi
    fi

    fail "install ROCm-enabled PyTorch or make hipcc available, then rerun this command"
}

start_workload() {
    local duration=$1 output=$2 ready=$3
    rm -f -- "$ready"
    "${WORKLOAD_CMD[@]}" "$duration" "$ready" >"$output" 2>&1 &
    WORKLOAD_PID=$!
    for _ in $(seq 1 300); do
        [[ -e "$ready" ]] && return 0
        kill -0 "$WORKLOAD_PID" 2>/dev/null || {
            wait "$WORKLOAD_PID" 2>/dev/null || true
            WORKLOAD_PID=""
            return 1
        }
        sleep 0.1
    done
    kill "$WORKLOAD_PID" 2>/dev/null || true
    wait "$WORKLOAD_PID" 2>/dev/null || true
    WORKLOAD_PID=""
    return 1
}

wait_workload() {
    local pid=$WORKLOAD_PID
    if ! wait "$pid"; then
        WORKLOAD_PID=""
        return 1
    fi
    WORKLOAD_PID=""
}

capture_environment() {
    local groups hip_version amd_version
    groups=" $(id -Gn) "
    hip_version="not installed"
    amd_version="not installed"
    if command -v hipcc >/dev/null 2>&1; then
        hip_version="$(hipcc --version 2>&1 | sed -n '1p' || true)"
        [[ -n "$hip_version" ]] || hip_version="unavailable"
    fi
    if command -v amd-smi >/dev/null 2>&1; then
        amd_version="$(timeout 5s amd-smi version 2>&1 | sed -n '1,20p' || true)"
        [[ -n "$amd_version" ]] || amd_version="unavailable"
    fi
    {
        printf 'kernel='; uname -srmo
        if [[ -r /etc/os-release ]]; then
            sed -n -E 's/^(ID|VERSION_ID|PRETTY_NAME)=/os_\1=/p' /etc/os-release
        fi
        printf 'render_group=%s\n' "$( [[ "$groups" == *' render '* ]] && printf yes || printf no )"
        printf 'video_group=%s\n' "$( [[ "$groups" == *' video '* ]] && printf yes || printf no )"
        printf 'workload=%s\n' "$WORKLOAD_KIND"
        if command -v python3 >/dev/null 2>&1; then
            printf 'python='; python3 --version 2>&1
        fi
        printf 'hipcc=%s\n' "$hip_version"
        printf '%s\n' "$amd_version" | sed 's/^/amd_smi=/'
    } > "$STAGING/environment.txt"
}

capture_drm_mapping() {
    printf 'render_node\tpci_bdf\tpci_id\tdriver\n' > "$STAGING/drm-mapping.tsv"
    for node in "${amdgpu_nodes[@]}"; do
        device="$(readlink -f "$node/device")"
        bdf="${device##*/}"
        pci_id=""
        if [[ -r "$node/device/uevent" ]]; then
            pci_id="$(sed -n 's/^PCI_ID=//p' "$node/device/uevent" | sed -n '1p')"
        fi
        printf '%s\t%s\t%s\tamdgpu\n' "${node##*/}" "$bdf" "$pci_id" >> "$STAGING/drm-mapping.tsv"
    done
}

capture_permissions() {
    printf 'path\tmode\tgroup_gid\treadable\twritable\n' > "$STAGING/permissions.tsv"
    paths=(/dev/kfd /dev/dri/renderD*)
    for path in "${paths[@]}"; do
        [[ -e "$path" ]] || continue
        mode="$(stat -c '%a' "$path")"
        group="$(stat -c '%g' "$path")"
        readable=no; writable=no
        [[ -r "$path" ]] && readable=yes
        [[ -w "$path" ]] && writable=yes
        printf '%s\t%s\t%s\t%s\t%s\n' "$path" "$mode" "$group" "$readable" "$writable" >> "$STAGING/permissions.tsv"
    done
}

capture_fdinfo() {
    local pid=$1 output=$2
    : > "$output"
    local files=(/proc/"$pid"/fdinfo/*)
    if ((${#files[@]} == 0)); then
        printf 'state\tfdinfo_unavailable\n' > "$output"
        return
    fi
    for file in "${files[@]}"; do
        [[ -r "$file" ]] || continue
        awk -v fd="${file##*/}" '
            /^(drm-|amd-|pasid)/ {
                key=$1; sub(/:$/, "", key)
                value=$2
                unit=(NF >= 3 ? $3 : "")
                printf "fd=%s\t%s\t%s\t%s\n", fd, key, value, unit
            }
        ' "$file"
    done | sort -u > "$output"
    [[ -s "$output" ]] || printf 'state\tno_drm_fdinfo_fields\n' > "$output"
}

capture_kfd() {
    local pid=$1 output=$2
    local root="/sys/class/kfd/kfd/proc/$pid"
    : > "$output"
    if [[ ! -e "$root" ]]; then
        printf 'state\tkfd_process_absent\n' > "$output"
        return
    fi
    if [[ ! -r "$root" || ! -x "$root" ]]; then
        printf 'state\tkfd_process_permission_denied\n' > "$output"
        return
    fi
    {
        while IFS= read -r -d '' file; do
            relative="${file#"$root"/}"
            printf 'file\t%s\n' "$relative"
            if [[ -r "$file" ]]; then
                awk '{print "value\t" $0}' "$file"
            else
                printf 'state\tpermission_denied\n'
            fi
        done < <(find "$root" -maxdepth 3 -type f -print0 2>/dev/null | sort -z)
    } > "$output"
    [[ -s "$output" ]] || printf 'state\tkfd_process_empty\n' > "$output"
}

full_process_scan() {
    local dir file
    # Measure the real process-list path: inspect readable fdinfo records, but
    # retain no fields from processes other than the harness-owned workload.
    for dir in /proc/[0-9]*/fdinfo; do
        [[ -r "$dir" ]] || continue
        for file in "$dir"/*; do
            [[ -r "$file" ]] || continue
            grep -qE '^(drm-|amd-|pasid)' "$file" 2>/dev/null || true
        done
    done
    if [[ -n "$WORKLOAD_PID" ]]; then
        for dir in "/sys/class/kfd/kfd/proc/$WORKLOAD_PID"; do
            [[ -r "$dir" ]] || continue
            for file in "$dir"/vram_* "$dir"/sdma_* "$dir"/stats_*/*; do
                [[ -r "$file" ]] || continue
                cat "$file" >/dev/null 2>&1 || true
            done
        done
    fi
}

measure_scans() {
    printf 'run\telapsed_ms\n' > "$STAGING/scan-timing.tsv"
    local run start end elapsed
    for run in $(seq 1 10); do
        start="$(date +%s%N)"
        full_process_scan
        end="$(date +%s%N)"
        elapsed="$(awk -v start="$start" -v end="$end" 'BEGIN { printf "%.3f", (end-start)/1000000 }')"
        printf '%s\t%s\n' "$run" "$elapsed" >> "$STAGING/scan-timing.tsv"
    done
}

extract_rate() {
    sed -n 's/^iterations_per_second=//p' "$1" | sed -n '1p'
}

run_benchmark() {
    local mode=$1 run=$2 table=$3
    local ready="$TMP_ROOT/${mode}-${run}.ready"
    local output="$TMP_ROOT/${mode}-${run}.out"
    start_workload 5 "$output" "$ready" || return 1
    if [[ "$mode" == "polled" ]]; then
        while kill -0 "$WORKLOAD_PID" 2>/dev/null; do
            full_process_scan
            sleep 2
        done
    fi
    wait_workload || return 1
    rate="$(extract_rate "$output")"
    [[ -n "$rate" ]] || return 1
    printf '%s\t%s\n' "$run" "$rate" >> "$table"
}

summarize() {
    local advancing association occupancy scan_mean baseline_mean polled_mean delta
    local has_drm=false has_kfd=false
    advancing="$(awk -F '\t' '
        NR==FNR { if ($2 ~ /^drm-engine-/) before[$1 FS $2]=$3+0; next }
        $2 ~ /^drm-engine-/ { key=$1 FS $2; if ((key in before) && $3+0 > before[key]) print $2 }
    ' "$STAGING/fdinfo-before.txt" "$STAGING/fdinfo-after.txt" | sort -u | paste -sd, -)"
    [[ -n "$advancing" ]] || advancing="none observed"

    grep -q $'\tdrm-pdev\t' "$STAGING/fdinfo-after.txt" && has_drm=true
    grep -q $'^file\t' "$STAGING/kfd-after.txt" && has_kfd=true
    if [[ "$has_drm" == true && "$has_kfd" == true ]]; then
        association="drm fdinfo, KFD process tree"
    elif [[ "$has_drm" == true ]]; then
        association="drm fdinfo"
    elif [[ "$has_kfd" == true ]]; then
        association="KFD process tree"
    else
        association="none observed"
    fi

    occupancy="not observed"
    grep -q 'cu_occupancy' "$STAGING/kfd-after.txt" && occupancy="present"

    scan_mean="$(awk -F '\t' 'NR>1 {sum+=$2; n++} END {if(n) printf "%.3f", sum/n; else print "not measured"}' "$STAGING/scan-timing.tsv")"
    baseline_mean="$(awk -F '\t' 'NR>1 {sum+=$2; n++} END {if(n) printf "%.6f", sum/n; else print "0"}' "$STAGING/workload-baseline.tsv")"
    polled_mean="$(awk -F '\t' 'NR>1 {sum+=$2; n++} END {if(n) printf "%.6f", sum/n; else print "0"}' "$STAGING/workload-polled.tsv")"
    delta="$(awk -v base="$baseline_mean" -v polled="$polled_mean" 'BEGIN {if(base>0) printf "%.3f", ((polled-base)/base)*100; else print "not measured"}')"

    {
        printf 'status=complete\n'
        printf 'workload=%s\n' "$WORKLOAD_KIND"
        printf 'association=%s\n' "$association"
        printf 'advancing_engine_fields=%s\n' "$advancing"
        printf 'kfd_cu_occupancy=%s\n' "$occupancy"
        printf 'cross_user_traversal=not tested\n'
        printf 'mean_full_scan_ms=%s\n' "$scan_mean"
        printf 'scan_context=workload_active\n'
        printf 'baseline_iterations_per_second=%s\n' "$baseline_mean"
        printf 'polled_iterations_per_second=%s\n' "$polled_mean"
        printf 'throughput_delta_percent=%s\n' "$delta"
        printf 'perturbation_budget=%s\n' "$(awk -v d="$delta" 'BEGIN {if(d=="not measured") print "not measured"; else if(d < -2.0) print "fail"; else print "pass"}')"
        printf 'privacy=hostname, username, command lines, serials, UUIDs, and unrelated process contents omitted\n'
    } > "$STAGING/summary.txt"
}

publish_results() {
    local archive="$TMP_ROOT/$RUN_STAMP.tar.gz"
    (
        cd "$STAGING"
        find . -type f ! -name manifest.sha256 -print0 | sort -z | xargs -0 sha256sum > manifest.sha256
    )
    tar -C "$TMP_ROOT" -czf "$archive" "$RUN_STAMP"
    mkdir -p "$RESULTS_ROOT"
    FINAL_RESULT="$RESULTS_ROOT/$RUN_STAMP"
    [[ ! -e "$FINAL_RESULT" && ! -e "$FINAL_RESULT.tar.gz" ]] || fail "result path already exists: $FINAL_RESULT"
    mv "$archive" "$FINAL_RESULT.tar.gz"
    mv "$STAGING" "$FINAL_RESULT"
}

select_workload
capture_environment
capture_drm_mapping
capture_permissions

printf 'Running %s workload and collecting process attribution evidence...\n' "$WORKLOAD_KIND"
main_ready="$TMP_ROOT/main.ready"
main_output="$TMP_ROOT/main.out"
start_workload 25 "$main_output" "$main_ready" || fail "the selected $WORKLOAD_KIND workload could not start"

capture_fdinfo "$WORKLOAD_PID" "$STAGING/fdinfo-before.txt"
capture_kfd "$WORKLOAD_PID" "$STAGING/kfd-before.txt"
sleep 2
capture_fdinfo "$WORKLOAD_PID" "$STAGING/fdinfo-after.txt"
capture_kfd "$WORKLOAD_PID" "$STAGING/kfd-after.txt"
diff -u "$STAGING/fdinfo-before.txt" "$STAGING/fdinfo-after.txt" > "$STAGING/fdinfo-diff.txt" || true
diff -u "$STAGING/kfd-before.txt" "$STAGING/kfd-after.txt" > "$STAGING/kfd-diff.txt" || true
measure_scans
wait_workload || fail "the evidence workload failed"
cp "$main_output" "$STAGING/workload-evidence.txt"

printf 'run\titerations_per_second\n' > "$STAGING/workload-baseline.tsv"
printf 'run\titerations_per_second\n' > "$STAGING/workload-polled.tsv"
for run in 1 2 3; do
    run_benchmark baseline "$run" "$STAGING/workload-baseline.tsv" || fail "baseline workload run $run failed"
done
for run in 1 2 3; do
    run_benchmark polled "$run" "$STAGING/workload-polled.tsv" || fail "polled workload run $run failed"
done

summarize
publish_results

if grep -q '^association=none observed$' "$FINAL_RESULT/summary.txt"; then
    FINAL_STATUS=1
fi

printf 'Capture complete: %s\n' "$FINAL_RESULT"
printf 'Transfer bundle: %s.tar.gz\n' "$FINAL_RESULT"
if ((FINAL_STATUS != 0)); then
    printf 'No fdinfo or KFD association evidence was observed; keep the bundle for review.\n' >&2
fi
exit "$FINAL_STATUS"

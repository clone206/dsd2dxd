#!/usr/bin/env bash
set -euo pipefail

# Benchmarks dsd2dxd by parsing "DSP speed: X.xx x" from its verbose output.
# Usage: ./bench_speed.sh [OUT_RATE] [IN_RATE] [INPUT_FILE]
# Defaults: OUT_RATE=192000, IN_RATE=2 (DSD128), INPUT_FILE attempts 1kHz_stereo_128.dsf or 1kHz_stereo_p.dsf

OUT_RATE=${1:-192000}
IN_RATE=${2:-2}
INPUT_FILE=${3:-}

if [[ -z "${INPUT_FILE}" ]]; then
  if [[ -f "1kHz_stereo_128.dsf" && ${IN_RATE} -eq 2 ]]; then
    INPUT_FILE="1kHz_stereo_128.dsf"
  elif [[ -f "1kHz_stereo_p.dsf" ]]; then
    INPUT_FILE="1kHz_stereo_p.dsf"
  else
    echo "Input file not found. Please pass an input file as arg 3." >&2
    exit 1
  fi
fi

extract_speed() {
  # Read from stdin, print first match of DSP speed numeric value (macOS-safe)
  perl -ne 'if (/DSP speed:\s*([0-9.]+)x/) { print "$1\n"; exit }'
}

run_case() {
  local label="$1"; shift
  # Remaining args are VAR=VALUE
  local envs=("$@")
  # Build env -i to avoid pollution
  local cmd=()
  for kv in "${envs[@]}"; do cmd+=("$kv"); done
  cmd+=(dsd2dxd -r "${OUT_RATE}" -i "${IN_RATE}" -o F -v "${INPUT_FILE}")
  # With -o F, output goes to a file; capture only stderr for diagnostics and parse directly
  local speed
  set +e
  speed="$(${cmd[@]} 2>&1 | perl -ne 'if (/DSP speed:\s*([0-9.]+)x/) { print "$1\n"; exit }')"
  local status=$?
  set -e
  # Cleanup generated file if present
  local base
  base=$(basename "$INPUT_FILE")
  base=${base%.*}
  rm -f "${base}.flac" 2>/dev/null || true
  if [[ $status -eq 0 && -n "${speed:-}" ]]; then
    echo "${label},${speed}"
  else
    echo "${label},NA"
  fi
}

# Print CSV header
printf "label,speed\n"

# 1) Sweep Stage1 target (ALIGN=M=21), Decim target fixed at 64 (ALIGN=7)
for t in 64 96 128 160 192 256; do
  run_case "stage1_target_${t}" \
    DSD2DXD_STAGE1_CHUNK_TARGET=${t} DSD2DXD_STAGE1_CHUNK_ALIGN=21 \
    DSD2DXD_DECIM_CHUNK_TARGET=64 DSD2DXD_DECIM_CHUNK_ALIGN=7
done

# 2) Sweep Stage1 MULT (ALIGN=21), Decim MULT=1 (ALIGN=7)
for m in 1 2 3 4; do
  run_case "stage1_mult_${m}" \
    DSD2DXD_STAGE1_CHUNK_MULT=${m} DSD2DXD_STAGE1_CHUNK_ALIGN=21 \
    DSD2DXD_DECIM_CHUNK_MULT=1 DSD2DXD_DECIM_CHUNK_ALIGN=7
done

# 3) Sweep Decim TARGET (ALIGN=7), Stage1 target=128 (ALIGN=21)
for dt in 32 48 64 96; do
  run_case "decim_target_${dt}" \
    DSD2DXD_STAGE1_CHUNK_TARGET=128 DSD2DXD_STAGE1_CHUNK_ALIGN=21 \
    DSD2DXD_DECIM_CHUNK_TARGET=${dt} DSD2DXD_DECIM_CHUNK_ALIGN=7
done

# 4) Sweep Decim MULT (ALIGN=7), Stage1 target=128 (ALIGN=21)
for dm in 1 2 4; do
  run_case "decim_mult_${dm}" \
    DSD2DXD_STAGE1_CHUNK_TARGET=128 DSD2DXD_STAGE1_CHUNK_ALIGN=21 \
    DSD2DXD_DECIM_CHUNK_MULT=${dm} DSD2DXD_DECIM_CHUNK_ALIGN=7
done

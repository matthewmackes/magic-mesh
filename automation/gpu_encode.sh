#!/bin/bash
set -eo pipefail
nvcc -arch=sm_75 -I /usr/local/cuda/include -c /root/magic-mesh/cudaencode.cu -o /tmp/cudaencode.o
nvcc -shared -o /tmp/cudaencode.so /tmp/cudaencode.o
mkdir -p /root/magic-mesh/shared/mde-egui/gpu-encoder
cp /tmp/cudaencode.so /root/magic-mesh/shared/mde-egui/gpu-encoder

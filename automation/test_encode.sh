#!/bin/bash
set -eo pipefail
mkdir -p /tmp/gpu_test
cp /root/magic-mesh/crates/shared/mde-egui/data/test.json /tmp/gpu_test/input.json
/bash automation/gpu_encode.sh
if [ $? -eq 0 ]; then
    echo "Success: GPU encoding completed"
    cp /tmp/gpu_test/ecoded.dat /root/magic-mesh/crates/shared/mde-egui/test/gpu-test.dat
    dash /root/magic-mesh/docs/visualization-template.he.SVG /root/magic-mesh/crates/shared/mde-egui/test/visualization.png
fi

# WL-CRIT-007 Syncthing registry amplification bound — r158

- Revision: `012961f0`
- Scope: the timer reconciler caps validated registry pairs before entering the Syncthing mutation loop; duplicate/hostile etcd output cannot amplify one run without bound.
- Gate: `bash install-helpers/test-syncthing-device-scope.sh` on BigBoy.
- Result: `PASS: Syncthing managed-folder device-scope self-test`, including the amplified duplicate-registry case with a cap of two entries.
- Live seat-15 observation remains bounded and read-only: Syncthing was ~20.6% CPU, shell ~10.8%, Music ~3.3%, with no daemon restart or throttle.


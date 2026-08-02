# SPICE Protocol Unimplemented Messages

## Common Messages (All Channels)
These messages apply to all channels but are not fully implemented:

### Partially Implemented
- `SPICE_MSG_SET_ACK` (3) - Implemented in cursor & display, missing in main & inputs
- `SPICE_MSG_NOTIFY` (7) - Implemented in main only
- `SPICE_MSG_DISCONNECTING` (6) - Implemented in main only

### Not Implemented
- `SPICE_MSG_MIGRATE` (1) - Migration support
- `SPICE_MSG_MIGRATE_DATA` (2) - Migration data transfer
- `SPICE_MSG_PING` (4) - Keepalive/latency measurement
- `SPICE_MSG_WAIT_FOR_CHANNELS` (5) - Channel synchronization

## Main Channel Messages

### Implemented
- `SPICE_MSG_MAIN_INIT` (103)
- `SPICE_MSG_MAIN_CHANNELS_LIST` (104)
- `SPICE_MSG_MAIN_MOUSE_MODE` (105)
- `SPICE_MSG_MAIN_MULTI_MEDIA_TIME` (106)
- `SPICE_MSG_MAIN_AGENT_CONNECTED` (107)
- `SPICE_MSG_MAIN_AGENT_DISCONNECTED` (108)
- `SPICE_MSG_MAIN_AGENT_DATA` (109)
- `SPICE_MSG_MAIN_AGENT_TOKEN` (110)

### Not Implemented
- `SPICE_MSG_MAIN_MIGRATE_BEGIN` (101)
- `SPICE_MSG_MAIN_MIGRATE_CANCEL` (102)
- `SPICE_MSG_MAIN_MIGRATE_SWITCH_HOST` (111)
- `SPICE_MSG_MAIN_MIGRATE_END` (112)
- `SPICE_MSG_MAIN_NAME` (113) - VM name
- `SPICE_MSG_MAIN_UUID` (114) - VM UUID
- `SPICE_MSG_MAIN_AGENT_CONNECTED_TOKENS` (115)
- `SPICE_MSG_MAIN_MIGRATE_BEGIN_SEAMLESS` (116)
- `SPICE_MSG_MAIN_MIGRATE_DST_SEAMLESS_ACK` (117)
- `SPICE_MSG_MAIN_MIGRATE_DST_SEAMLESS_NACK` (118)

## Display Channel Messages

### Implemented
- `SPICE_MSG_DISPLAY_MODE` (101)
- `SPICE_MSG_DISPLAY_RESET` (103)
- `SPICE_MSG_DISPLAY_COPY_BITS` (104) - Surface region copy operations
- `SPICE_MSG_DISPLAY_SURFACE_CREATE` (318)
- `SPICE_MSG_DISPLAY_SURFACE_DESTROY` (319)
- `SPICE_MSG_DISPLAY_MONITORS_CONFIG` (320)
- Draw messages (302-317, 321) - Partially implemented
- Stream messages (122-126) - Partially implemented

### Not Implemented
- `SPICE_MSG_DISPLAY_MARK` (102) - Used for synchronization
- `SPICE_MSG_DISPLAY_INVAL_LIST` (105) - Already stubbed but no implementation
- `SPICE_MSG_DISPLAY_INVAL_ALL_PIXMAPS` (106) - Already stubbed but no implementation
- `SPICE_MSG_DISPLAY_INVAL_PALETTE` (107)
- `SPICE_MSG_DISPLAY_INVAL_ALL_PALETTES` (108)

## Cursor Channel Messages

### Implemented
- `SPICE_MSG_SET_ACK` (3)
- `SPICE_MSG_CURSOR_INIT` (101)
- `SPICE_MSG_CURSOR_SET` (103)
- `SPICE_MSG_CURSOR_MOVE` (104)
- `SPICE_MSG_CURSOR_HIDE` (105)
- `SPICE_MSG_CURSOR_TRAIL` (106)
- `SPICE_MSG_CURSOR_INVAL_ONE` (107)
- `SPICE_MSG_CURSOR_INVAL_ALL` (108)

### Not Implemented
- `SPICE_MSG_CURSOR_RESET` (102) - Reset cursor to default

## Inputs Channel Messages

### Server to Client - Implemented
- `SPICE_MSG_INPUTS_INIT` (101)
- `SPICE_MSG_INPUTS_KEY_MODIFIERS` (102)

### Server to Client - Not Implemented
- Common messages (SET_ACK, PING, etc.)

### Client to Server Messages (Not applicable for this task)
- `SPICE_MSG_INPUTS_KEY_DOWN` (103)
- `SPICE_MSG_INPUTS_KEY_UP` (104)
- `SPICE_MSG_INPUTS_MOUSE_MOTION` (105)
- `SPICE_MSG_INPUTS_MOUSE_POSITION` (106)
- `SPICE_MSG_INPUTS_MOUSE_PRESS` (107)
- `SPICE_MSG_INPUTS_MOUSE_RELEASE` (108)

## Priority Recommendations

### High Priority (Core functionality)
1. `SPICE_MSG_PING` - Essential for connection health monitoring
2. `SPICE_MSG_SET_ACK` - Complete implementation in all channels
3. `SPICE_MSG_MAIN_NAME` and `SPICE_MSG_MAIN_UUID` - VM identification
4. `SPICE_MSG_CURSOR_RESET` - Basic cursor functionality
5. `SPICE_MSG_DISPLAY_MARK` - Display synchronization

### Medium Priority (Enhanced features)
1. `SPICE_MSG_WAIT_FOR_CHANNELS` - Multi-channel sync
2. `SPICE_MSG_DISPLAY_INVAL_LIST` - Cache invalidation
3. `SPICE_MSG_DISPLAY_INVAL_*` - Cache invalidation

### Low Priority (Advanced features)
1. Migration messages - Not critical for basic VM interaction
2. Seamless migration - Advanced enterprise feature
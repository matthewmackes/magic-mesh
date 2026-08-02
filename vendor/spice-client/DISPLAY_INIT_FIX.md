# Display Channel Initialization Fix - Investigation Needed

## Problem
The SPICE client was failing to display anything when connecting to both simple SPICE test servers and QEMU. The error was:
```
DisplayChannel: No primary surface available. Total surfaces: 0
```

## Current "Fix"
We added code to create a default primary surface (ID 0) immediately after sending `SPICE_MSGC_DISPLAY_INIT`:

```rust
// Create default primary surface (ID 0) with a reasonable default size
// Many SPICE servers assume surface 0 exists after display init
let mut surfaces = HashMap::new();
let default_width = 1024;
let default_height = 768;
let default_format = 32; // 32-bit RGBA

info!("Creating default primary surface (ID 0) - {}x{}", default_width, default_height);
surfaces.insert(
    0,
    DisplaySurface {
        width: default_width,
        height: default_height,
        format: default_format,
        data: vec![0; (default_width * default_height * 4) as usize],
    },
);
```

## Why This Fix is Suspicious

1. **Protocol Violation**: According to the SPICE protocol, surfaces should be created by the server sending `SPICE_MSG_DISPLAY_SURFACE_CREATE` (type 318) or `SPICE_MSG_DISPLAY_MODE` (type 101) messages. The client shouldn't pre-create surfaces.

2. **Hardcoded Dimensions**: We're using hardcoded 1024x768 dimensions which may not match the actual display size the server wants.

3. **Works by Accident**: This fix only works because:
   - Some SPICE servers assume surface 0 exists without explicitly creating it
   - The servers are sending draw commands to surface 0 without first sending a surface creation message

## What Should Actually Happen

According to the SPICE protocol reference:

1. Client connects display channel and sends `SPICE_MSGC_DISPLAY_INIT`
2. Server should respond with either:
   - `SPICE_MSG_DISPLAY_MODE` (101) - Sets up primary display with dimensions
   - `SPICE_MSG_DISPLAY_SURFACE_CREATE` (318) - Creates a surface with specific ID

3. Only after receiving these messages should the client create surfaces

## Evidence from Tests

From the test logs:
- Server is immediately sending `SPICE_MSG_DISPLAY_DRAW_COPY` (type 304) messages
- No `SPICE_MSG_DISPLAY_MODE` or `SPICE_MSG_DISPLAY_SURFACE_CREATE` messages were received
- Server assumes surface 0 exists without creating it

## Proper Fix Investigation Needed

1. **Check other SPICE clients**: How do spice-gtk and other clients handle this? Do they also pre-create surface 0?

2. **Protocol clarification**: Is there an implicit surface 0 creation in the protocol that's not well documented?

3. **Server behavior**: Why are both the test server and QEMU not sending surface creation messages?

4. **Capability negotiation**: Are we missing some capability flags that would make the server send proper initialization?

## Temporary Nature

This fix should be considered **temporary** and needs proper investigation. The comment in the code acknowledges this:
```rust
// Many SPICE servers assume surface 0 exists after display init
```

But this might be masking a deeper protocol implementation issue.

## Next Steps

1. Study spice-gtk implementation for display initialization
2. Add protocol tracing to see exact message sequence
3. Test with different SPICE server versions
4. Consider if we're missing some initialization step
5. Check if there's a "legacy mode" vs "surface mode" in SPICE protocol
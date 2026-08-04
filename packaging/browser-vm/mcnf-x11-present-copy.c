/*
 * Force wlroots' nested X11 backend to use Present COPY semantics.
 *
 * xorgxrdp captures Xorg's software screen backing pixmap.  A normal Present
 * flip leaves that backing pixmap stale even though the nested Sway window is
 * visibly composed by Xorg.  COPY keeps the presented pixels in the drawable
 * that xorgxrdp captures.  The interposer is inert unless explicitly enabled.
 */
#define _GNU_SOURCE

#include <dlfcn.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <xcb/present.h>

typedef xcb_void_cookie_t (*present_pixmap_fn)(
    xcb_connection_t *connection,
    xcb_window_t window,
    xcb_pixmap_t pixmap,
    uint32_t serial,
    xcb_xfixes_region_t valid,
    xcb_xfixes_region_t update,
    int16_t x_offset,
    int16_t y_offset,
    xcb_randr_crtc_t target_crtc,
    xcb_sync_fence_t wait_fence,
    xcb_sync_fence_t idle_fence,
    uint32_t options,
    uint64_t target_msc,
    uint64_t divisor,
    uint64_t remainder,
    uint32_t notifies_len,
    const xcb_present_notify_t *notifies);

static present_pixmap_fn real_present_pixmap;

struct copy_state {
    xcb_connection_t *connection;
    xcb_window_t window;
    xcb_window_t root;
    xcb_gcontext_t gc;
    uint16_t width;
    uint16_t height;
};

static struct copy_state copy_state;

static void resolve_present_pixmap(void)
{
    void *symbol = dlsym(RTLD_NEXT, "xcb_present_pixmap");

    if (symbol == NULL || sizeof(symbol) != sizeof(real_present_pixmap)) {
        _exit(127);
    }

    memcpy(&real_present_pixmap, &symbol, sizeof(real_present_pixmap));
}

static void prepare_root_copy(xcb_connection_t *connection,
    xcb_window_t window, xcb_pixmap_t pixmap)
{
    xcb_get_geometry_cookie_t geometry_cookie;
    xcb_get_geometry_reply_t *geometry;
    xcb_void_cookie_t gc_cookie;
    xcb_generic_error_t *error = NULL;
    uint32_t gc_values[] = {XCB_SUBWINDOW_MODE_INCLUDE_INFERIORS};

    if (copy_state.connection == connection && copy_state.window == window) {
        return;
    }

    geometry_cookie = xcb_get_geometry(connection, pixmap);
    geometry = xcb_get_geometry_reply(connection, geometry_cookie, &error);
    if (geometry == NULL || error != NULL || geometry->width == 0 ||
        geometry->height == 0) {
        free(error);
        free(geometry);
        _exit(127);
    }

    copy_state.connection = connection;
    copy_state.window = window;
    copy_state.root = geometry->root;
    copy_state.width = geometry->width;
    copy_state.height = geometry->height;
    copy_state.gc = xcb_generate_id(connection);
    free(geometry);

    gc_cookie = xcb_create_gc_checked(connection, copy_state.gc,
        copy_state.root, XCB_GC_SUBWINDOW_MODE, gc_values);
    error = xcb_request_check(connection, gc_cookie);
    if (error != NULL) {
        free(error);
        _exit(127);
    }
}

xcb_void_cookie_t xcb_present_pixmap(
    xcb_connection_t *connection,
    xcb_window_t window,
    xcb_pixmap_t pixmap,
    uint32_t serial,
    xcb_xfixes_region_t valid,
    xcb_xfixes_region_t update,
    int16_t x_offset,
    int16_t y_offset,
    xcb_randr_crtc_t target_crtc,
    xcb_sync_fence_t wait_fence,
    xcb_sync_fence_t idle_fence,
    uint32_t options,
    uint64_t target_msc,
    uint64_t divisor,
    uint64_t remainder,
    uint32_t notifies_len,
    const xcb_present_notify_t *notifies)
{
    const char *enabled = getenv("MCNF_X11_PRESENT_COPY");
    xcb_void_cookie_t present_cookie;

    if (real_present_pixmap == NULL) {
        resolve_present_pixmap();
    }

    if (enabled != NULL && strcmp(enabled, "1") == 0) {
        options |= XCB_PRESENT_OPTION_COPY;
    }

    present_cookie = real_present_pixmap(connection, window, pixmap, serial, valid,
        update, x_offset, y_offset, target_crtc, wait_fence, idle_fence,
        options, target_msc, divisor, remainder, notifies_len, notifies);

    if (enabled != NULL && strcmp(enabled, "1") == 0) {
        prepare_root_copy(connection, window, pixmap);
        (void)xcb_copy_area(connection, pixmap, copy_state.root, copy_state.gc,
            0, 0, 0, 0, copy_state.width, copy_state.height);
        (void)xcb_flush(connection);
    }

    return present_cookie;
}

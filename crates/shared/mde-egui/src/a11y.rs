//! The runtime **AccessKit consumer seam** (a11y-01).
//!
//! egui/eframe only generate an AccessKit accessibility tree once
//! [`egui::Context::enable_accesskit`] has been called, and only *emit* it as
//! `full_output.platform_output.accesskit_update` on the frames the tree is rebuilt.
//! The bare-DRM seat ([`crate::run_drm`]) drives its own present loop with a plain
//! [`egui::Context`] and no eframe/winit adapter, so nothing enabled AccessKit or
//! drained that update in production — the rich per-widget annotation work throughout
//! the shell (live regions, roles, labels) was reachable *only* from `#[cfg(test)]`
//! code (bug **a11y-01**). The shipped seat therefore exported an empty accessibility
//! tree.
//!
//! This module is the missing runtime plumbing:
//! * [`AccessKitSink`] — the consumer trait the present loop drains each frame into.
//!   a11y-02's screen reader implements it (an `accesskit_consumer` tree-walker + local
//!   TTS); see `docs/design/accessibility.md`.
//! * [`LatestTree`] — the default sink: it just retains the most-recent tree update and
//!   exposes it (that is what a11y-01's runtime self-test reads to prove the stream is
//!   live).
//! * [`A11yBridge`] — the loop-facing façade [`crate::run_drm`] builds. It compiles to a
//!   genuine no-op when the crate is built without the `accesskit` feature, so the
//!   present loop body carries no `#[cfg]`.
//!
//! Enablement is gated (default **OFF**) so nothing changes for seats that don't opt
//! in, but it is reachable in production — not test-only — via [`A11yBridge::from_env`].

/// Environment variable that opts a bare-DRM seat into AccessKit tree generation.
///
/// Set `MDE_A11Y=1` (any non-empty value other than `0`) before launching the shell to
/// have [`crate::run_drm`] call [`egui::Context::enable_accesskit`] at startup and drain
/// the tree every frame into the [`AccessKitSink`]. This mirrors the `MDE_DRM_ESC_QUIT`
/// env idiom already used in the same present loop. Unset (the default) leaves AccessKit
/// off — zero cost. a11y-02 adds a live hotkey toggle that flips the same switch (it
/// persists a seat accessibility setting and enables AccessKit on the running context).
pub const A11Y_ENV: &str = "MDE_A11Y";

/// Whether the environment opts this seat into AccessKit (see [`A11Y_ENV`]).
#[must_use]
pub fn a11y_requested() -> bool {
    std::env::var_os(A11Y_ENV).is_some_and(|v| {
        let v = v.to_string_lossy();
        !v.is_empty() && v != "0"
    })
}

// ── the consumer seam (only meaningful when AccessKit is compiled in) ─────────────
#[cfg(feature = "accesskit")]
pub use enabled::{AccessKitSink, LatestTree};

#[cfg(feature = "accesskit")]
mod enabled {
    use egui::accesskit::TreeUpdate;

    /// The consumer seam a downstream accessibility client plugs into.
    ///
    /// [`crate::run_drm`] drains each rendered frame's AccessKit tree into the sink. The
    /// default [`LatestTree`] merely retains the latest tree; a11y-02's screen reader
    /// implements [`AccessKitSink`] over an `accesskit_consumer::Tree`, walks the focus /
    /// live-region deltas, and speaks them through local TTS.
    pub trait AccessKitSink {
        /// Consume the AccessKit tree update produced by the frame just rendered.
        fn ingest(&mut self, update: TreeUpdate);

        /// Whether the consumer wants a fresh frame rendered *now* — e.g. an AT client
        /// just connected / requested the tree, or the reader needs a re-scan. It is
        /// polled once per present-loop iteration; when `true`, the loop forces a render
        /// (mirroring perf-1's `force_render`) so the idle-sleep can never starve the
        /// exported tree. The default never asks for a refresh — a pure latest-tree
        /// holder does not drive rendering.
        fn wants_refresh(&mut self) -> bool {
            false
        }
    }

    /// The default sink: holds the most-recent [`TreeUpdate`] and a one-shot refresh
    /// request.
    ///
    /// a11y-01's runtime self-test reads [`LatestTree::latest`] to prove the production
    /// stream is live; a11y-02 replaces this with the real screen reader.
    #[derive(Default)]
    pub struct LatestTree {
        latest: Option<TreeUpdate>,
        refresh_requested: bool,
    }

    impl LatestTree {
        /// The most-recent tree update drained from the present loop, if a frame has been
        /// drained since construction.
        #[must_use]
        pub const fn latest(&self) -> Option<&TreeUpdate> {
            self.latest.as_ref()
        }

        /// Ask the present loop to render a fresh frame on its next iteration (so the
        /// exported tree can't go stale while the loop is otherwise idle). Consumed by
        /// [`AccessKitSink::wants_refresh`] on the next poll.
        pub const fn request_refresh(&mut self) {
            self.refresh_requested = true;
        }
    }

    impl AccessKitSink for LatestTree {
        fn ingest(&mut self, update: TreeUpdate) {
            self.latest = Some(update);
        }

        fn wants_refresh(&mut self) -> bool {
            std::mem::take(&mut self.refresh_requested)
        }
    }
}

// ── the loop-facing façade: real plumbing with `accesskit`, no-op without ─────────

/// The present loop's AccessKit façade with the `accesskit` feature **on**.
///
/// It turns egui's tree generation on when enabled, drains each frame's tree into an
/// [`AccessKitSink`], and surfaces the sink's refresh requests as a render wake.
#[cfg(feature = "accesskit")]
pub struct A11yBridge {
    enabled: bool,
    sink: Box<dyn AccessKitSink>,
}

/// The present loop's AccessKit façade with the `accesskit` feature **off**: a
/// zero-cost no-op with the same method surface, so [`crate::run_drm`] compiles and
/// runs identically whether or not the accessibility stack is built in.
#[cfg(not(feature = "accesskit"))]
pub struct A11yBridge;

#[cfg(feature = "accesskit")]
impl A11yBridge {
    /// Build the bridge from the environment ([`A11Y_ENV`]); AccessKit defaults **off**
    /// and the sink is the default [`LatestTree`].
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            enabled: a11y_requested(),
            sink: Box::new(LatestTree::default()),
        }
    }

    /// Build an **enabled** bridge around a caller-provided sink — the seam a11y-02's
    /// screen reader plugs into. Only available when the `accesskit` feature is compiled
    /// in.
    #[must_use]
    pub fn with_sink(sink: Box<dyn AccessKitSink>) -> Self {
        Self {
            enabled: true,
            sink,
        }
    }

    /// Whether AccessKit tree generation is on for this seat.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Turn AccessKit generation on for `ctx` if this bridge is enabled. Called once at
    /// present-loop startup; a no-op when disabled.
    pub fn enable(&self, ctx: &egui::Context) {
        if self.enabled {
            ctx.enable_accesskit();
        }
    }

    /// Whether the consumer wants a fresh render this iteration, so the loop can fold it
    /// into `force_render`. Always `false` when disabled.
    pub fn wants_render(&mut self) -> bool {
        self.enabled && self.sink.wants_refresh()
    }

    /// Drain this frame's AccessKit tree update (if any) into the sink. A no-op when
    /// disabled. Takes the update out of `platform_output`, leaving the caller's later
    /// moves of `full_output` (shapes / textures) untouched.
    pub fn drain(&mut self, full_output: &mut egui::FullOutput) {
        if self.enabled {
            if let Some(update) = full_output.platform_output.accesskit_update.take() {
                self.sink.ingest(update);
            }
        }
    }
}

#[cfg(not(feature = "accesskit"))]
#[allow(
    clippy::unused_self,
    clippy::missing_const_for_fn,
    clippy::needless_pass_by_ref_mut
)]
impl A11yBridge {
    /// Build the (no-op) bridge — AccessKit is not compiled in.
    #[must_use]
    pub const fn from_env() -> Self {
        Self
    }

    /// Always `false` — AccessKit is not compiled in.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        false
    }

    /// No-op — AccessKit is not compiled in.
    pub fn enable(&self, _ctx: &egui::Context) {}

    /// Always `false` — AccessKit is not compiled in.
    pub fn wants_render(&mut self) -> bool {
        false
    }

    /// No-op — AccessKit is not compiled in.
    pub fn drain(&mut self, _full_output: &mut egui::FullOutput) {}
}

#[cfg(all(test, feature = "accesskit"))]
mod tests {
    use super::{A11yBridge, AccessKitSink, LatestTree, A11Y_ENV};

    /// Render one real egui frame that draws a couple of labeled widgets and return the
    /// `FullOutput`. Used to obtain a genuine, non-synthetic `TreeUpdate`.
    fn render_labeled_frame(ctx: &egui::Context) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(320.0, 240.0),
            )),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("hello a11y");
                let _ = ui.button("press me");
            });
        })
    }

    #[test]
    fn latest_tree_holds_the_latest_update_and_refresh_is_one_shot() {
        let mut sink = LatestTree::default();
        assert!(sink.latest().is_none(), "no tree before any ingest");
        assert!(!sink.wants_refresh(), "no refresh requested by default");
        sink.request_refresh();
        assert!(sink.wants_refresh(), "requested refresh surfaces once");
        assert!(
            !sink.wants_refresh(),
            "refresh request is consumed exactly once"
        );

        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        crate::Style::install(&ctx);
        let out = render_labeled_frame(&ctx);
        let update = out
            .platform_output
            .accesskit_update
            .expect("accesskit enabled → Some(update)");
        let nodes = update.nodes.len();
        assert!(nodes >= 2, "a labeled frame yields a root + widget nodes");
        sink.ingest(update);
        assert_eq!(
            sink.latest().map(|u| u.nodes.len()),
            Some(nodes),
            "the ingested tree is retained"
        );
    }

    #[test]
    fn enabled_bridge_flows_a_real_frame_through_a_pluggable_sink() {
        use std::cell::Cell;
        use std::rc::Rc;

        // A recording sink standing in for a11y-02's screen reader — it proves the seam
        // is genuinely pluggable and that a real tree flows through it.
        struct Probe {
            frames: Rc<Cell<usize>>,
            nodes: Rc<Cell<usize>>,
            want: Rc<Cell<bool>>,
        }
        impl AccessKitSink for Probe {
            fn ingest(&mut self, update: egui::accesskit::TreeUpdate) {
                self.frames.set(self.frames.get() + 1);
                self.nodes.set(update.nodes.len());
            }
            fn wants_refresh(&mut self) -> bool {
                self.want.replace(false)
            }
        }

        let frames = Rc::new(Cell::new(0));
        let nodes = Rc::new(Cell::new(0));
        let want = Rc::new(Cell::new(false));
        let mut bridge = A11yBridge::with_sink(Box::new(Probe {
            frames: frames.clone(),
            nodes: nodes.clone(),
            want: want.clone(),
        }));
        assert!(bridge.is_enabled(), "with_sink is enabled");

        let ctx = egui::Context::default();
        crate::Style::install(&ctx);
        // The bridge — not the test — enables AccessKit, exactly as run_drm does.
        bridge.enable(&ctx);
        let mut out = render_labeled_frame(&ctx);
        assert!(
            out.platform_output.accesskit_update.is_some(),
            "bridge.enable() turned tree generation on"
        );

        bridge.drain(&mut out);
        assert_eq!(frames.get(), 1, "exactly one frame drained into the sink");
        assert!(
            nodes.get() >= 2,
            "a non-trivial tree flowed through the seam"
        );
        assert!(
            out.platform_output.accesskit_update.is_none(),
            "drain took the update out of platform_output"
        );

        // Wake-freshness: a consumer refresh request surfaces as one render wake.
        assert!(!bridge.wants_render(), "no refresh pending");
        want.set(true);
        assert!(
            bridge.wants_render(),
            "a requested refresh wakes a render once"
        );
        assert!(!bridge.wants_render(), "the refresh wake is one-shot");
    }

    #[test]
    fn from_env_defaults_off_and_is_inert() {
        // The farm/CI never sets MDE_A11Y; assert the default-off contract and that the
        // disabled bridge is a genuine no-op (guarded so an opted-in shell env can't fail
        // it).
        if std::env::var_os(A11Y_ENV).is_some() {
            return;
        }
        let mut bridge = A11yBridge::from_env();
        assert!(!bridge.is_enabled(), "MDE_A11Y unset → AccessKit off");

        let ctx = egui::Context::default();
        crate::Style::install(&ctx);
        bridge.enable(&ctx);
        let mut out = render_labeled_frame(&ctx);
        assert!(
            out.platform_output.accesskit_update.is_none(),
            "a disabled bridge generates no tree"
        );
        bridge.drain(&mut out); // harmless
        assert!(!bridge.wants_render());
    }
}

use clap::Parser;
use spice_client::SpiceClientShared;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::signal;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(author, version, about = "SPICE client end-to-end test program", long_about = None)]
struct Args {
    #[arg(short = 'H', long, default_value = "localhost")]
    host: String,

    #[arg(short, long, default_value = "5900")]
    port: u16,

    #[arg(short = 'd', long, default_value = "30")]
    duration: u64,

    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[arg(short = 'P', long)]
    password: Option<String>,

    #[arg(long, help = "Fail if no display updates are received")]
    require_display_updates: bool,

    #[arg(long, help = "Fail if display channel is not connected")]
    require_display_channel: bool,

    #[arg(long, help = "Test mouse input events")]
    test_mouse_input: bool,

    #[arg(long, help = "Test keyboard input events")]
    test_keyboard_input: bool,

    #[arg(long, default_value = "5", help = "Connection timeout in seconds")]
    connect_timeout: u64,

    #[arg(long, help = "Save protocol trace on failure")]
    trace_on_failure: bool,

    #[arg(
        long,
        help = "Progress report interval in seconds",
        default_value = "5"
    )]
    progress_interval: u64,

    #[arg(
        long,
        help = "Minimum display updates required for success",
        default_value = "0"
    )]
    min_display_updates: u32,

    #[arg(long, help = "Exit immediately on first error")]
    fail_fast: bool,
}

struct TestMetrics {
    display_updates_received: AtomicU32,
    display_channel_connected: AtomicBool,
    inputs_channel_connected: AtomicBool,
    cursor_channel_connected: AtomicBool,
    main_channel_connected: AtomicBool,
    last_display_width: AtomicU32,
    last_display_height: AtomicU32,
    errors_encountered: AtomicU32,
    warnings_encountered: AtomicU32,
    test_start_time: Instant,
    protocol_events: Arc<tokio::sync::Mutex<Vec<String>>>,
}

impl TestMetrics {
    fn new() -> Self {
        Self {
            display_updates_received: AtomicU32::new(0),
            display_channel_connected: AtomicBool::new(false),
            inputs_channel_connected: AtomicBool::new(false),
            cursor_channel_connected: AtomicBool::new(false),
            main_channel_connected: AtomicBool::new(false),
            last_display_width: AtomicU32::new(0),
            last_display_height: AtomicU32::new(0),
            errors_encountered: AtomicU32::new(0),
            warnings_encountered: AtomicU32::new(0),
            test_start_time: Instant::now(),
            protocol_events: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    async fn log_event(&self, event: String) {
        let mut events = self.protocol_events.lock().await;
        let elapsed = self.test_start_time.elapsed();
        events.push(format!("[{:>6.2}s] {}", elapsed.as_secs_f32(), event));
    }

    async fn print_summary(&self) {
        let duration = self.test_start_time.elapsed();
        info!("=== E2E Test Summary ===");
        info!("Test duration: {:.2}s", duration.as_secs_f32());
        info!(
            "Display updates received: {}",
            self.display_updates_received.load(Ordering::Relaxed)
        );
        info!(
            "Channels connected: Main={}, Display={}, Inputs={}, Cursor={}",
            self.main_channel_connected.load(Ordering::Relaxed),
            self.display_channel_connected.load(Ordering::Relaxed),
            self.inputs_channel_connected.load(Ordering::Relaxed),
            self.cursor_channel_connected.load(Ordering::Relaxed)
        );
        info!(
            "Last display size: {}x{}",
            self.last_display_width.load(Ordering::Relaxed),
            self.last_display_height.load(Ordering::Relaxed)
        );
        info!(
            "Errors: {}, Warnings: {}",
            self.errors_encountered.load(Ordering::Relaxed),
            self.warnings_encountered.load(Ordering::Relaxed)
        );
        info!("=======================");
    }

    async fn save_protocol_trace(&self, filename: &str) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs::File;
        use std::io::Write;

        let events = self.protocol_events.lock().await;
        let mut file = File::create(filename)?;

        writeln!(file, "SPICE E2E Test Protocol Trace")?;
        writeln!(file, "=============================")?;
        writeln!(file, "Start time: {:?}", self.test_start_time)?;
        writeln!(
            file,
            "Duration: {:.2}s\n",
            self.test_start_time.elapsed().as_secs_f32()
        )?;

        for event in events.iter() {
            writeln!(file, "{}", event)?;
        }

        info!("Protocol trace saved to: {}", filename);
        Ok(())
    }

    fn check_success_criteria(&self, args: &Args) -> Result<(), String> {
        let mut failures = Vec::new();

        if !self.main_channel_connected.load(Ordering::Relaxed) {
            failures.push("Main channel never connected".to_string());
        }

        if args.require_display_channel && !self.display_channel_connected.load(Ordering::Relaxed) {
            failures.push("Display channel was required but not connected".to_string());
        }

        let display_updates = self.display_updates_received.load(Ordering::Relaxed);
        if args.require_display_updates && display_updates == 0 {
            failures.push("Display updates were required but none received".to_string());
        }

        if display_updates < args.min_display_updates {
            failures.push(format!(
                "Insufficient display updates: got {}, required minimum {}",
                display_updates, args.min_display_updates
            ));
        }

        let errors = self.errors_encountered.load(Ordering::Relaxed);
        if errors > 0 {
            failures.push(format!("{} errors encountered during test", errors));
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

async fn test_mouse_movements(
    client: &SpiceClientShared,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Testing mouse input events...");

    // Test pattern: move mouse in a square pattern
    let positions = vec![(100, 100), (500, 100), (500, 500), (100, 500), (100, 100)];

    for (x, y) in positions {
        debug!("Moving mouse to ({}, {})", x, y);
        client.send_mouse_motion(0, x, y).await?;
        sleep(Duration::from_millis(100)).await;
    }

    // Test mouse clicks
    use spice_client::channels::MouseButton;
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        debug!("Testing {:?} mouse button", button);
        client.send_mouse_button(0, button, true).await?;
        sleep(Duration::from_millis(50)).await;
        client.send_mouse_button(0, button, false).await?;
        sleep(Duration::from_millis(50)).await;
    }

    // Test mouse wheel
    debug!("Testing mouse wheel");
    client.send_mouse_wheel(0, 0, 5).await?;
    sleep(Duration::from_millis(50)).await;
    client.send_mouse_wheel(0, 0, -5).await?;

    info!("Mouse input tests completed");
    Ok(())
}

async fn test_keyboard_input(client: &SpiceClientShared) -> Result<(), Box<dyn std::error::Error>> {
    info!("Testing keyboard input events...");

    // Test some common scancodes (A-Z keys)
    let test_keys = vec![
        (0x1E, "A"),      // A key
        (0x30, "B"),      // B key
        (0x2E, "C"),      // C key
        (0x20, "D"),      // D key
        (0x12, "E"),      // E key
        (0x01, "Escape"), // Escape key
        (0x1C, "Enter"),  // Enter key
        (0x39, "Space"),  // Space key
    ];

    for (scancode, key_name) in test_keys {
        debug!("Testing key: {} (scancode: 0x{:02X})", key_name, scancode);
        client.send_key_down(0, scancode).await?;
        sleep(Duration::from_millis(50)).await;
        client.send_key_up(0, scancode).await?;
        sleep(Duration::from_millis(50)).await;
    }

    // Test key combination (Ctrl+A)
    debug!("Testing key combination: Ctrl+A");
    client.send_key_down(0, 0x1D).await?; // Ctrl down
    sleep(Duration::from_millis(50)).await;
    client.send_key_down(0, 0x1E).await?; // A down
    sleep(Duration::from_millis(50)).await;
    client.send_key_up(0, 0x1E).await?; // A up
    sleep(Duration::from_millis(50)).await;
    client.send_key_up(0, 0x1D).await?; // Ctrl up

    info!("Keyboard input tests completed");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Setup logging
    let log_level = match args.verbose {
        0 => tracing::Level::INFO,
        1 => tracing::Level::DEBUG,
        _ => tracing::Level::TRACE,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    info!("Starting SPICE E2E test client");
    info!("Test configuration:");
    info!("  Host: {}:{}", args.host, args.port);
    info!("  Duration: {} seconds", args.duration);
    info!(
        "  Require display updates: {}",
        args.require_display_updates
    );
    info!(
        "  Require display channel: {}",
        args.require_display_channel
    );
    info!("  Test mouse input: {}", args.test_mouse_input);
    info!("  Test keyboard input: {}", args.test_keyboard_input);
    info!("  Connect timeout: {} seconds", args.connect_timeout);
    info!("  Progress interval: {} seconds", args.progress_interval);
    info!("  Minimum display updates: {}", args.min_display_updates);
    info!("  Fail fast: {}", args.fail_fast);
    info!("  Trace on failure: {}", args.trace_on_failure);

    let metrics = Arc::new(TestMetrics::new());
    let mut client = SpiceClientShared::new(args.host.clone(), args.port);

    // Set up graceful shutdown handler
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        match signal::ctrl_c().await {
            Ok(()) => {
                warn!("Received interrupt signal, initiating graceful shutdown...");
                shutdown_clone.store(true, Ordering::Relaxed);
            }
            Err(err) => error!("Unable to listen for shutdown signal: {}", err),
        }
    });

    if let Some(ref password) = args.password {
        info!("Setting password");
        client.set_password(password.clone()).await;
    }

    // Connect to SPICE server with timeout
    let connect_result = timeout(Duration::from_secs(args.connect_timeout), client.connect()).await;

    match connect_result {
        Ok(Ok(())) => {
            info!("✓ Successfully connected to SPICE server");
            metrics
                .main_channel_connected
                .store(true, Ordering::Relaxed);
            metrics
                .log_event("Main channel connected successfully".to_string())
                .await;

            // Check which channels are connected
            info!("Checking channel connections...");
            let start_check = Instant::now();
            while start_check.elapsed() < Duration::from_secs(2)
                && !shutdown.load(Ordering::Relaxed)
            {
                if client.get_display_surface(0).await.is_some() {
                    metrics
                        .display_channel_connected
                        .store(true, Ordering::Relaxed);
                    metrics
                        .log_event("Display channel 0 connected".to_string())
                        .await;
                    info!("✓ Display channel 0 is connected");
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }

            // Set up display update callback
            let metrics_clone = metrics.clone();
            match client
                .set_display_update_callback(0, move |surface| {
                    let count = metrics_clone
                        .display_updates_received
                        .fetch_add(1, Ordering::Relaxed)
                        + 1;
                    metrics_clone
                        .last_display_width
                        .store(surface.width, Ordering::Relaxed);
                    metrics_clone
                        .last_display_height
                        .store(surface.height, Ordering::Relaxed);
                    debug!(
                        "Display update #{}: {}x{}, format: {:?}",
                        count, surface.width, surface.height, surface.format
                    );
                })
                .await
            {
                Ok(()) => info!("✓ Display update callback registered"),
                Err(e) => warn!("Could not set display callback: {}", e),
            }

            // Start event loops
            match client.start_event_loop().await {
                Ok(()) => {
                    info!("✓ Event loops started successfully");
                    metrics.log_event("Event loops started".to_string()).await;

                    // Run input tests if requested
                    if args.test_mouse_input {
                        if let Err(e) = test_mouse_movements(&client).await {
                            error!("Mouse input tests failed: {}", e);
                            metrics.errors_encountered.fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    if args.test_keyboard_input {
                        if let Err(e) = test_keyboard_input(&client).await {
                            error!("Keyboard input tests failed: {}", e);
                            metrics.errors_encountered.fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    // Monitor for the specified duration with progress reporting
                    info!("Starting test run for {} seconds...", args.duration);
                    let start = Instant::now();
                    let mut last_report = Instant::now();
                    let mut last_update_count = 0u32;

                    while start.elapsed().as_secs() < args.duration
                        && !shutdown.load(Ordering::Relaxed)
                    {
                        // Progress reporting
                        if last_report.elapsed() >= Duration::from_secs(args.progress_interval) {
                            let elapsed = start.elapsed().as_secs();
                            let remaining = args.duration.saturating_sub(elapsed);
                            let current_updates =
                                metrics.display_updates_received.load(Ordering::Relaxed);
                            let update_rate = (current_updates - last_update_count) as f32
                                / args.progress_interval as f32;

                            info!(
                                "[{}/{}s] Updates: {} ({:.1}/s), Errors: {}, Display: {}x{}",
                                elapsed,
                                args.duration,
                                current_updates,
                                update_rate,
                                metrics.errors_encountered.load(Ordering::Relaxed),
                                metrics.last_display_width.load(Ordering::Relaxed),
                                metrics.last_display_height.load(Ordering::Relaxed)
                            );

                            last_update_count = current_updates;
                            last_report = Instant::now();

                            // Check for early termination conditions
                            if args.fail_fast
                                && metrics.errors_encountered.load(Ordering::Relaxed) > 0
                            {
                                error!("Fail-fast triggered due to errors");
                                break;
                            }
                        }

                        // Check display surface
                        if let Some(surface) = client.get_display_surface(0).await {
                            metrics
                                .last_display_width
                                .store(surface.width, Ordering::Relaxed);
                            metrics
                                .last_display_height
                                .store(surface.height, Ordering::Relaxed);
                        }

                        // Check cursor shape
                        if let Some(cursor) = client.get_cursor_shape(0).await {
                            if !metrics.cursor_channel_connected.load(Ordering::Relaxed) {
                                metrics
                                    .cursor_channel_connected
                                    .store(true, Ordering::Relaxed);
                                info!("✓ Cursor channel 0 is connected");
                                debug!(
                                    "Cursor shape: {}x{}, hotspot: ({}, {})",
                                    cursor.width,
                                    cursor.height,
                                    cursor.hot_spot_x,
                                    cursor.hot_spot_y
                                );
                            }
                        }

                        sleep(Duration::from_millis(100)).await;
                    }

                    // Graceful shutdown
                    if shutdown.load(Ordering::Relaxed) {
                        info!("Test interrupted by user, performing graceful shutdown...");
                        metrics
                            .log_event("Test interrupted by user signal".to_string())
                            .await;
                    } else {
                        info!("Test duration complete, disconnecting...");
                        metrics
                            .log_event("Test duration completed normally".to_string())
                            .await;
                    }

                    // Disconnect with timeout to ensure we don't hang
                    match timeout(Duration::from_secs(3), client.disconnect()).await {
                        Ok(()) => {
                            info!("✓ Disconnected successfully");
                            metrics
                                .log_event("Disconnected successfully".to_string())
                                .await;
                        }
                        Err(_) => {
                            warn!("Disconnect timeout after 3 seconds");
                            metrics.log_event("Disconnect timeout".to_string()).await;
                        }
                    }

                    // Print final metrics
                    metrics.print_summary().await;

                    // Check success criteria
                    match metrics.check_success_criteria(&args) {
                        Ok(()) => {
                            info!("✓ All E2E tests PASSED");
                            Ok(())
                        }
                        Err(failure_reason) => {
                            error!("✗ E2E tests FAILED: {}", failure_reason);

                            if args.trace_on_failure {
                                let trace_file = format!(
                                    "e2e_failure_{}.trace",
                                    chrono::Local::now().format("%Y%m%d_%H%M%S")
                                );
                                let _ = metrics.save_protocol_trace(&trace_file).await;
                            }

                            Err("E2E tests FAILED".into())
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to start event loop: {}", e);
                    Err(e.into())
                }
            }
        }
        Ok(Err(e)) => {
            error!("Failed to connect to SPICE server: {}", e);
            metrics.log_event(format!("Connection failed: {}", e)).await;

            if e.to_string().contains("BAD_CONNECTION_ID") {
                error!("Protocol error: BAD_CONNECTION_ID - Check connection_id handling");
            } else if e.to_string().contains("refused") {
                error!(
                    "Connection refused - Is the SPICE server running on {}:{}?",
                    args.host, args.port
                );
            }

            if args.trace_on_failure {
                let _ = metrics
                    .save_protocol_trace("e2e_connect_failure.trace")
                    .await;
            }

            Err(e.into())
        }
        Err(_) => {
            error!("Connection timeout after {} seconds", args.connect_timeout);
            metrics
                .log_event(format!(
                    "Connection timeout after {}s",
                    args.connect_timeout
                ))
                .await;

            if args.trace_on_failure {
                let _ = metrics.save_protocol_trace("e2e_timeout.trace").await;
            }

            Err(format!("Connection timeout to {}:{}", args.host, args.port).into())
        }
    }
}

//! Guest-local Browser probe page.
//!
//! The page has no synthetic sample source. Playback uses a WebAudio oscillator
//! created by a trusted click. Capture invokes getUserMedia synchronously from a
//! trusted click, records its real MediaStream through WebAudio, and uploads the
//! resulting PCM to the guest controller. A second trusted click starts every
//! measured operation after the collector's start signal.

use crate::protocol::JobSpec;

pub struct RenderedPage {
    pub html: Vec<u8>,
    pub csp: String,
}

#[must_use]
pub fn render(spec: &JobSpec, script_nonce: &str) -> RenderedPage {
    let operation = spec.operation.as_str();
    let html = PAGE_TEMPLATE
        .replace("__SCRIPT_NONCE__", script_nonce)
        .replace("__JOB_ID__", &spec.job_id)
        .replace("__OPERATION__", operation)
        .replace("__TONE_HZ__", &spec.tone_hz.to_string())
        .replace("__DURATION_SECONDS__", &spec.duration_seconds.to_string());
    RenderedPage {
        html: html.into_bytes(),
        csp: format!(
            "default-src 'none'; script-src 'nonce-{script_nonce}'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'none'; media-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"
        ),
    }
}

const PAGE_TEMPLATE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>MCNF Browser VM Audio Control</title>
<style>
  :root { color-scheme: dark; font-family: system-ui, sans-serif; }
  * { box-sizing: border-box; }
  body { margin: 0; min-height: 100vh; display: grid; place-items: center;
         background: #111318; color: #f6f7fb; }
  main { width: min(760px, 86vw); text-align: center; }
  h1 { font-size: 34px; margin: 0 0 16px; }
  p { color: #c9ced8; font-size: 18px; }
  button { min-width: 420px; min-height: 180px; margin-top: 30px; border: 3px solid #fff;
           border-radius: 28px; background: #b4232c; color: white; font: 700 28px system-ui;
           cursor: pointer; box-shadow: 0 20px 60px #0008; }
  button:focus { outline: 8px solid #ffcf5c; outline-offset: 8px; }
  #status { min-height: 28px; margin-top: 30px; font: 600 17px ui-monospace, monospace; }
</style>
</head>
<body>
<main>
  <h1>Browser-owned audio qualification</h1>
  <p>Every arm and measured start requires an observed RDP click.</p>
  <button id="control" type="button">Arm browser audio</button>
  <div id="status" role="status">Loading one-time control…</div>
</main>
<script nonce="__SCRIPT_NONCE__">
(() => {
  "use strict";
  const jobId = "__JOB_ID__";
  const operation = "__OPERATION__";
  const toneHz = __TONE_HZ__;
  const durationSeconds = __DURATION_SECONDS__;
  const expectedRate = 48000;
  const expectedChannels = 2;
  const base = `/probe/${jobId}`;
  const button = document.getElementById("control");
  const status = document.getElementById("status");
  let state = "loading";
  let context = null;
  let stream = null;
  let source = null;
  let processor = null;
  let silentGain = null;
  let recording = false;
  let recordedFrames = 0;
  let leftChunks = [];
  let rightChunks = [];
  let captureStartedAt = 0;
  let firstCaptureCallback = false;

  const setStatus = (value) => { status.textContent = value; };
  const trustedGesture = (event) => event.isTrusted === true &&
    navigator.userActivation && navigator.userActivation.isActive === true;

  async function postEvent(payload) {
    const response = await fetch(`${base}/event`, {
      method: "POST",
      cache: "no-store",
      credentials: "omit",
      headers: {"Content-Type": "application/json"},
      body: JSON.stringify(payload)
    });
    if (!response.ok) throw new Error(`event-${response.status}`);
  }

  async function fail(code) {
    state = "failed";
    recording = false;
    button.disabled = true;
    setStatus(`Probe failed closed: ${code}`);
    try { await postEvent({event: "failed", reason_code: String(code).slice(0, 80)}); }
    catch (_) { /* The controller will retain the last trustworthy state. */ }
  }

  function requireContext() {
    if (!context || context.state !== "running" || context.sampleRate !== expectedRate ||
        context.destination.channelCount < expectedChannels) {
      throw new Error("webaudio-contract");
    }
  }

  async function armPlayback(event) {
    const gesture = trustedGesture(event);
    if (!gesture) throw new Error("untrusted-arm-click");
    context = new AudioContext({sampleRate: expectedRate, latencyHint: "interactive"});
    await context.resume();
    requireContext();
    await postEvent({
      event: "playback_armed",
      is_trusted: event.isTrusted === true,
      user_activation: gesture,
      audio_context_state: context.state,
      sample_rate: context.sampleRate,
      channels: expectedChannels
    });
    state = "armed";
    button.textContent = `Play ${toneHz} Hz test tone`;
    setStatus("WebAudio armed by trusted RDP click; waiting for collector start.");
  }

  async function startPlayback(event) {
    const gesture = trustedGesture(event);
    if (!gesture) throw new Error("untrusted-start-click");
    requireContext();
    const oscillator = context.createOscillator();
    const gain = context.createGain();
    oscillator.type = "sine";
    oscillator.frequency.setValueAtTime(toneHz, context.currentTime);
    gain.gain.setValueAtTime(0.25, context.currentTime);
    oscillator.connect(gain).connect(context.destination);
    const began = performance.now();
    oscillator.start();
    oscillator.stop(context.currentTime + durationSeconds);
    await postEvent({
      event: "playback_started",
      is_trusted: event.isTrusted === true,
      user_activation: gesture,
      audio_context_state: context.state,
      sample_rate: context.sampleRate,
      channels: expectedChannels
    });
    state = "running";
    button.disabled = true;
    setStatus("Real Browser WebAudio tone is active.");
    oscillator.onended = async () => {
      try {
        const elapsed = Math.round(performance.now() - began);
        await postEvent({event: "playback_completed", oscillator_ended: true, elapsed_ms: elapsed});
        state = "completed";
        setStatus("Browser WebAudio tone completed.");
        await context.close();
      } catch (error) { await fail(error.message || "playback-completion"); }
    };
  }

  function encodeStereoPcm16(frameCount) {
    const output = new ArrayBuffer(44 + frameCount * 4);
    const view = new DataView(output);
    const ascii = (offset, value) => {
      for (let index = 0; index < value.length; index += 1) {
        view.setUint8(offset + index, value.charCodeAt(index));
      }
    };
    ascii(0, "RIFF");
    view.setUint32(4, 36 + frameCount * 4, true);
    ascii(8, "WAVE");
    ascii(12, "fmt ");
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, expectedChannels, true);
    view.setUint32(24, expectedRate, true);
    view.setUint32(28, expectedRate * expectedChannels * 2, true);
    view.setUint16(32, expectedChannels * 2, true);
    view.setUint16(34, 16, true);
    ascii(36, "data");
    view.setUint32(40, frameCount * 4, true);
    let offset = 44;
    let remaining = frameCount;
    for (let chunk = 0; chunk < leftChunks.length && remaining > 0; chunk += 1) {
      const count = Math.min(remaining, leftChunks[chunk].length);
      for (let frame = 0; frame < count; frame += 1) {
        const left = Math.max(-1, Math.min(1, leftChunks[chunk][frame]));
        const right = Math.max(-1, Math.min(1, rightChunks[chunk][frame]));
        view.setInt16(offset, left < 0 ? left * 32768 : left * 32767, true);
        view.setInt16(offset + 2, right < 0 ? right * 32768 : right * 32767, true);
        offset += 4;
      }
      remaining -= count;
    }
    if (remaining !== 0) throw new Error("capture-frame-underflow");
    return output;
  }

  async function uploadCapture() {
    const expectedFrames = expectedRate * durationSeconds;
    const wav = encodeStereoPcm16(expectedFrames);
    const response = await fetch(`${base}/wav`, {
      method: "POST",
      cache: "no-store",
      credentials: "omit",
      headers: {"Content-Type": "audio/wav"},
      body: wav
    });
    if (!response.ok) throw new Error(`wav-${response.status}`);
    await postEvent({
      event: "capture_completed",
      frames: expectedFrames,
      sample_rate: expectedRate,
      channels: expectedChannels,
      elapsed_ms: Math.round(performance.now() - captureStartedAt)
    });
    state = "completed";
    setStatus("Browser getUserMedia PCM captured; waiting for collector release.");
    pollRelease();
  }

  async function pollRelease() {
    if (state !== "completed") return;
    try {
      const response = await fetch(`${base}/command`, {
        method: "GET", cache: "no-store", credentials: "omit"
      });
      if (!response.ok) throw new Error(`command-${response.status}`);
      const command = await response.json();
      if (command.command === "release") {
        if (processor) processor.disconnect();
        if (source) source.disconnect();
        if (silentGain) silentGain.disconnect();
        if (stream) stream.getTracks().forEach((track) => track.stop());
        if (context && context.state !== "closed") await context.close();
        await postEvent({event: "released"});
        state = "released";
        setStatus("Microphone released after collector route restoration.");
        return;
      }
    } catch (error) {
      await fail(error.message || "release-poll");
      return;
    }
    setTimeout(pollRelease, 100);
  }

  async function armCapture(event) {
    const gesture = trustedGesture(event);
    if (!gesture) throw new Error("untrusted-gum-click");
    // Invocation happens synchronously inside the trusted click handler. Do not
    // move this call below an await: the user-activation provenance is material.
    const streamPromise = navigator.mediaDevices.getUserMedia({audio: {
      channelCount: {exact: expectedChannels},
      sampleRate: {ideal: expectedRate},
      echoCancellation: false,
      noiseSuppression: false,
      autoGainControl: false
    }, video: false});
    stream = await streamPromise;
    const tracks = stream.getAudioTracks();
    if (tracks.length !== 1 || tracks[0].kind !== "audio" || tracks[0].readyState !== "live") {
      throw new Error("microphone-track-contract");
    }
    const settings = tracks[0].getSettings();
    if (Number(settings.channelCount || 0) !== expectedChannels) {
      throw new Error("microphone-not-stereo");
    }
    context = new AudioContext({sampleRate: expectedRate, latencyHint: "interactive"});
    await context.resume();
    requireContext();
    source = context.createMediaStreamSource(stream);
    processor = context.createScriptProcessor(4096, expectedChannels, expectedChannels);
    silentGain = context.createGain();
    silentGain.gain.setValueAtTime(0, context.currentTime);
    source.connect(processor).connect(silentGain).connect(context.destination);
    processor.onaudioprocess = async (audioEvent) => {
      try {
        const input = audioEvent.inputBuffer;
        if (input.numberOfChannels < expectedChannels || input.sampleRate !== expectedRate) {
          throw new Error("capture-buffer-contract");
        }
        if (!firstCaptureCallback) {
          firstCaptureCallback = true;
          await postEvent({
            event: "capture_ready",
            is_trusted: event.isTrusted === true,
            user_activation: gesture,
            audio_context_state: context.state,
            media_track_kind: tracks[0].kind,
            media_track_state: tracks[0].readyState,
            sample_rate: input.sampleRate,
            channels: expectedChannels
          });
          state = "armed";
          button.textContent = "Start real microphone capture";
          setStatus("getUserMedia is live from a trusted RDP click; waiting for collector start.");
          return;
        }
        if (!recording) return;
        leftChunks.push(new Float32Array(input.getChannelData(0)));
        rightChunks.push(new Float32Array(input.getChannelData(1)));
        recordedFrames += input.length;
        if (recordedFrames >= expectedRate * durationSeconds) {
          recording = false;
          await uploadCapture();
        }
      } catch (error) { await fail(error.message || "capture-callback"); }
    };
  }

  async function startCapture(event) {
    const gesture = trustedGesture(event);
    if (!gesture) throw new Error("untrusted-capture-start");
    requireContext();
    if (!stream || stream.getAudioTracks().some((track) => track.readyState !== "live")) {
      throw new Error("microphone-not-live-at-start");
    }
    leftChunks = [];
    rightChunks = [];
    recordedFrames = 0;
    captureStartedAt = performance.now();
    recording = true;
    await postEvent({
      event: "capture_started",
      is_trusted: event.isTrusted === true,
      user_activation: gesture,
      audio_context_state: context.state,
      sample_rate: context.sampleRate,
      channels: expectedChannels
    });
    state = "running";
    button.disabled = true;
    setStatus("Real Browser getUserMedia capture is active.");
  }

  button.addEventListener("click", async (event) => {
    try {
      if (state === "loaded") {
        if (operation === "playback") await armPlayback(event);
        else if (operation === "capture") await armCapture(event);
        else throw new Error("operation-contract");
      } else if (state === "armed") {
        if (operation === "playback") await startPlayback(event);
        else await startCapture(event);
      }
    } catch (error) { await fail(error.message || "control-click"); }
  });

  (async () => {
    try {
      await postEvent({event: "page_loaded"});
      state = "loaded";
      button.focus();
      setStatus("One-time Browser control loaded; trusted RDP arm click required.");
    } catch (error) { await fail(error.message || "page-load"); }
  })();
})();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::render;
    use crate::protocol::{JobSpec, Operation};

    fn spec(operation: Operation) -> JobSpec {
        JobSpec {
            schema_version: 1,
            job_id: "a".repeat(64),
            operation,
            phase: "before-recovery".to_owned(),
            tone_hz: 523,
            duration_seconds: if operation == Operation::Playback {
                8
            } else {
                2
            },
            source_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            image_digest: format!("sha256:{}", "b".repeat(64)),
            transport: "rdp".to_owned(),
        }
    }

    #[test]
    fn page_requires_trusted_activation_and_real_browser_apis() {
        let playback =
            String::from_utf8(render(&spec(Operation::Playback), "nonce").html).unwrap_or_default();
        assert!(playback.contains("event.isTrusted === true"));
        assert!(playback.contains("navigator.userActivation.isActive === true"));
        assert!(playback.contains("context.createOscillator()"));
        assert!(!playback.contains("autoplay-policy"));

        let capture =
            String::from_utf8(render(&spec(Operation::Capture), "nonce").html).unwrap_or_default();
        assert!(capture.contains("navigator.mediaDevices.getUserMedia"));
        assert!(capture.contains("createMediaStreamSource"));
        assert!(capture.contains("createScriptProcessor"));
        assert!(capture.contains("body: wav"));
    }

    #[test]
    fn csp_keeps_page_network_and_script_surface_bounded() {
        let page = render(&spec(Operation::Playback), "abcdef");
        assert!(page.csp.contains("script-src 'nonce-abcdef'"));
        assert!(page.csp.contains("connect-src 'self'"));
        assert!(page.csp.contains("default-src 'none'"));
    }
}

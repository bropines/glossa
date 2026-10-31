//! Playing the speech that text-to-speech services return (MP3, WAV, Ogg
//! Vorbis) without blocking the caller.
//!
//! The output device is opened lazily on the first `play` and released after
//! [`DEFAULT_IDLE_TIMEOUT`] of silence: an open audio stream keeps the sound
//! server busy, can stop the system from sleeping, and holds a few MB of
//! buffers — for a program that speaks a sentence now and then and otherwise
//! sits in the tray.
//!
//! Playback lives on a worker thread that owns the device (audio streams are
//! not `Send` on every platform). The thread exits with the device, and the
//! next `play` starts a new one.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::{platform_err, Result};

/// How long the device stays open after the last sound finished.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the worker checks whether playback has ended.
const TICK: Duration = Duration::from_millis(50);

enum Command {
    Play(Vec<u8>, SyncSender<Result<()>>),
    Stop,
}

struct Shared {
    /// The worker's inbox; `None` while no worker runs. Held locked while
    /// sending, and by the worker while it retires, so a command can never
    /// be sent to a thread that is about to exit.
    tx: Mutex<Option<Sender<Command>>>,
    playing: AtomicBool,
    idle_timeout: Duration,
}

impl Shared {
    fn inbox(&self) -> MutexGuard<'_, Option<Sender<Command>>> {
        self.tx.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A non-blocking player for encoded audio. Cheap to create; nothing is
/// opened until the first [`play`](Self::play).
pub struct AudioPlayer {
    shared: Arc<Shared>,
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for AudioPlayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioPlayer").field("playing", &self.is_playing()).finish()
    }
}

impl AudioPlayer {
    pub fn new() -> Self {
        Self::with_idle_timeout(DEFAULT_IDLE_TIMEOUT)
    }

    /// A player that releases the device after `idle` of silence.
    pub fn with_idle_timeout(idle: Duration) -> Self {
        AudioPlayer {
            shared: Arc::new(Shared { tx: Mutex::new(None), playing: AtomicBool::new(false), idle_timeout: idle }),
        }
    }

    /// Start playing `bytes` (MP3, WAV or Ogg Vorbis), replacing whatever is
    /// playing. Returns once playback has started, or with an error if the
    /// data cannot be decoded or no output device can be opened.
    pub fn play(&self, bytes: Vec<u8>) -> Result<()> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.send(Command::Play(bytes, reply_tx));
        match reply_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => Err(platform_err("the audio device did not respond")),
        }
    }

    /// Stop playback. The device stays open until the idle timeout.
    pub fn stop(&self) {
        if self.shared.inbox().is_some() {
            self.send(Command::Stop);
        }
        self.shared.playing.store(false, Ordering::Relaxed);
    }

    /// Whether something is audible right now (updated within ~50 ms of the
    /// end of playback).
    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    fn send(&self, cmd: Command) {
        let mut inbox = self.shared.inbox();
        let cmd = match inbox.as_ref() {
            Some(tx) => match tx.send(cmd) {
                Ok(()) => return,
                // The worker died (it never should); start another.
                Err(mpsc::SendError(cmd)) => cmd,
            },
            None => cmd,
        };
        let (tx, rx) = mpsc::channel();
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new().name("glossa-audio".into()).spawn(move || worker(shared, rx));
        match spawned {
            Ok(_) => {
                let _ = tx.send(cmd);
                *inbox = Some(tx);
            }
            Err(e) => {
                if let Command::Play(_, reply) = cmd {
                    let _ = reply.send(Err(platform_err(format!("cannot start audio thread: {e}"))));
                }
            }
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        // Dropping the sender ends the worker, which drops the device.
        self.shared.inbox().take();
    }
}

/// The device and the current sound, owned by the worker thread.
#[derive(Default)]
struct Output {
    device: Option<rodio::MixerDeviceSink>,
    player: Option<rodio::Player>,
}

impl Output {
    fn play(&mut self, bytes: Vec<u8>) -> Result<()> {
        // Decode first: bad data should fail without touching the device.
        let source = rodio::Decoder::try_from(Cursor::new(bytes))
            .map_err(|e| platform_err(format!("cannot decode audio: {e}")))?;
        if self.device.is_none() {
            let mut device = rodio::DeviceSinkBuilder::open_default_sink()
                .map_err(|e| platform_err(format!("cannot open the audio device: {e}")))?;
            device.log_on_drop(false);
            self.device = Some(device);
        }
        if let Some(old) = self.player.take() {
            old.stop();
        }
        let Some(device) = self.device.as_ref() else {
            return Err(platform_err("audio device unavailable"));
        };
        let player = rodio::Player::connect_new(device.mixer());
        player.append(source);
        self.player = Some(player);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            p.stop();
        }
    }

    fn busy(&self) -> bool {
        self.player.as_ref().is_some_and(|p| !p.empty())
    }
}

fn worker(shared: Arc<Shared>, rx: Receiver<Command>) {
    let mut out = Output::default();
    let mut idle_since = Instant::now();
    let mut pending: Option<Command> = None;
    loop {
        let next = match pending.take() {
            Some(cmd) => Ok(cmd),
            None => rx.recv_timeout(TICK),
        };
        match next {
            Ok(Command::Play(bytes, reply)) => {
                let result = out.play(bytes);
                shared.playing.store(result.is_ok(), Ordering::Relaxed);
                let _ = reply.send(result);
            }
            Ok(Command::Stop) => {
                out.stop();
                shared.playing.store(false, Ordering::Relaxed);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        if out.busy() {
            idle_since = Instant::now();
            continue;
        }
        out.player = None;
        shared.playing.store(false, Ordering::Relaxed);
        if idle_since.elapsed() < shared.idle_timeout {
            continue;
        }
        // Retire, unless a command slipped in before we took the lock.
        let mut inbox = shared.inbox();
        match rx.try_recv() {
            Ok(cmd) => {
                pending = Some(cmd);
                idle_since = Instant::now();
            }
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => {
                *inbox = None;
                return; // drops the device
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid 16-bit mono PCM WAV of `samples` silent samples.
    fn wav(samples: u32) -> Vec<u8> {
        let data_len = samples * 2;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes()); // PCM
        v.extend_from_slice(&1u16.to_le_bytes()); // mono
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_len.to_le_bytes());
        v.resize(v.len() + data_len as usize, 0);
        v
    }

    #[test]
    fn wav_decodes_with_the_enabled_features() {
        assert!(rodio::Decoder::try_from(Cursor::new(wav(800))).is_ok());
    }

    #[test]
    fn garbage_is_rejected_without_opening_a_device() {
        let p = AudioPlayer::new();
        let err = p.play(b"definitely not audio".to_vec()).unwrap_err();
        assert!(err.to_string().contains("decode"), "{err}");
        assert!(!p.is_playing());
        p.stop();
    }

    #[test]
    fn idle_worker_retires_and_a_new_one_starts_on_demand() {
        let p = AudioPlayer::with_idle_timeout(Duration::from_millis(50));
        assert!(p.play(vec![1, 2, 3]).is_err());
        assert!(p.shared.inbox().is_some());
        let deadline = Instant::now() + Duration::from_secs(5);
        while p.shared.inbox().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(p.shared.inbox().is_none(), "worker should have retired");
        // And the next request still gets an answer.
        assert!(p.play(vec![4, 5, 6]).is_err());
    }

    #[test]
    #[ignore = "needs an audio output device"]
    fn plays_a_short_wav() {
        let p = AudioPlayer::new();
        p.play(wav(4000)).unwrap();
        assert!(p.is_playing());
        std::thread::sleep(Duration::from_millis(800));
        assert!(!p.is_playing());
    }
}

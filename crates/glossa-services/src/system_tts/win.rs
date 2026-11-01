//! The system voice on Windows: WinRT `Windows.Media.SpeechSynthesis`, the
//! voices of Settings → Time & language → Speech. `SynthesizeTextToStreamAsync`
//! returns a WAV stream, read here into memory.

use std::time::{Duration, Instant};

use glossa_core::{Error, Result, SpeakRequest};
use glossa_modules::CancelToken;
use windows::core::{Interface, HSTRING};
use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows_future::{AsyncStatus, IAsyncOperation};

use super::{no_voice, pick_voice, timeout_for, Voice, SLOW_RATE, SYSTEM_TTS_ID};

const HOW_TO_ADD: &str = "Add one in Settings → Time & language → Speech (Manage voices), or install the language \
                          with its speech feature in Language & region.";

pub struct Backend;

impl Backend {
    pub fn new() -> Self {
        Backend
    }

    pub fn description(&self) -> String {
        format!("Reads text aloud with the voices installed in Windows, offline. {HOW_TO_ADD}")
    }

    pub fn homepage(&self) -> &'static str {
        "https://learn.microsoft.com/uwp/api/windows.media.speechsynthesis"
    }

    /// The speech API is part of Windows 10 and 11; a missing voice is
    /// reported when one is asked for.
    pub fn unavailable(&self) -> Option<String> {
        None
    }

    pub fn speak(&self, req: &SpeakRequest, cancel: &CancelToken) -> Result<Vec<u8>> {
        let _apartment = Apartment::enter();
        let synth = SpeechSynthesizer::new().map_err(|e| fail("cannot start speech synthesis", e))?;
        if req.lang.is_language() {
            let installed: Vec<VoiceInformation> =
                SpeechSynthesizer::AllVoices().map_err(|e| fail("cannot list the voices", e))?.into_iter().collect();
            let voices: Vec<Voice> = installed
                .iter()
                .map(|v| Voice {
                    id: v.Id().map(|s| s.to_string()).unwrap_or_default(),
                    name: v.DisplayName().map(|s| s.to_string()).unwrap_or_default(),
                    tag: v.Language().map(|s| s.to_string()).unwrap_or_default(),
                    penalty: 0,
                })
                .collect();
            let default_id = SpeechSynthesizer::DefaultVoice().and_then(|v| v.Id()).map(|s| s.to_string()).ok();
            let default = default_id.and_then(|id| voices.iter().position(|v| v.id == id));
            let i = pick_voice(&voices, req.lang, default).ok_or_else(|| no_voice(req.lang, &voices, HOW_TO_ADD))?;
            synth.SetVoice(&installed[i]).map_err(|e| fail("cannot choose the voice", e))?;
        }
        if req.slow {
            // SpeakingRate needs Windows 10 1803; older ones speak at normal speed.
            if let Err(e) = synth.Options().and_then(|o| o.SetSpeakingRate(SLOW_RATE)) {
                tracing::debug!("cannot slow the system voice down: {}", e.message());
            }
        }
        let op = synth
            .SynthesizeTextToStreamAsync(&HSTRING::from(req.text.as_str()))
            .map_err(|e| fail("speech synthesis failed", e))?;
        let stream = wait(&op, cancel, timeout_for(&req.text)).map_err(|e| e.or("speech synthesis failed"))?;
        let size = stream.Size().map_err(|e| fail("cannot read the audio", e))?;
        let size = u32::try_from(size).map_err(|_| Error::bad_response(super::NAME, "the audio is too large"))?;
        let input = stream.GetInputStreamAt(0).map_err(|e| fail("cannot read the audio", e))?;
        let reader = DataReader::CreateDataReader(&input).map_err(|e| fail("cannot read the audio", e))?;
        let load = reader
            .LoadAsync(size)
            .and_then(|op| op.cast::<IAsyncOperation<u32>>())
            .map_err(|e| fail("cannot read the audio", e))?;
        let n = wait(&load, cancel, Duration::from_secs(30)).map_err(|e| e.or("cannot read the audio"))?;
        let mut bytes = vec![0u8; n as usize];
        reader.ReadBytes(&mut bytes).map_err(|e| fail("cannot read the audio", e))?;
        let _ = stream.Close();
        Ok(bytes)
    }
}

/// Why waiting for a WinRT operation stopped.
enum Wait {
    Done(Error),
    Failed(windows::core::Error),
}

impl Wait {
    fn or(self, what: &str) -> Error {
        match self {
            Wait::Done(e) => e,
            Wait::Failed(e) => fail(what, e),
        }
    }
}

/// The result of `op`, polling so a cancel or the deadline can stop it.
fn wait<T: windows::core::RuntimeType + 'static>(
    op: &IAsyncOperation<T>,
    cancel: &CancelToken,
    timeout: Duration,
) -> std::result::Result<T, Wait> {
    let deadline = Instant::now() + timeout;
    loop {
        match op.Status() {
            Ok(AsyncStatus::Started) => {}
            Ok(_) => return op.join().map_err(Wait::Failed),
            Err(e) => return Err(Wait::Failed(e)),
        }
        if cancel.is_cancelled() {
            let _ = op.Cancel();
            return Err(Wait::Done(Error::Cancelled));
        }
        if Instant::now() >= deadline {
            let _ = op.Cancel();
            return Err(Wait::Done(Error::Timeout(timeout.as_millis() as u64)));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fail(what: &str, e: windows::core::Error) -> Error {
    Error::Module { module: SYSTEM_TTS_ID.into(), detail: format!("{what}: {} (0x{:08X})", e.message(), e.code().0) }
}

/// Joins the calling thread to the multithreaded apartment while it lives,
/// as WinRT needs; engine jobs run on short-lived threads.
struct Apartment {
    joined: bool,
}

impl Apartment {
    fn enter() -> Apartment {
        // S_OK and S_FALSE both need a matching CoUninitialize; failure (the
        // thread is already single-threaded) needs none and WinRT still works.
        let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        Apartment { joined }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.joined {
            unsafe { CoUninitialize() };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{fix_wav, voice_language};
    use super::*;
    use glossa_core::LangId;

    /// Speaks through Windows with the first installed voice's language and
    /// with none, and checks the answers are WAV. Run with `--ignored` on
    /// Windows.
    #[test]
    #[ignore = "speaks through Windows"]
    fn live_windows_voice_speaks_wav() {
        let tags: Vec<String> = {
            let _apartment = Apartment::enter();
            SpeechSynthesizer::AllVoices().unwrap().into_iter().map(|v| v.Language().unwrap().to_string()).collect()
        };
        eprintln!("installed voices: {tags:?}");
        let lang = tags.iter().find_map(|t| voice_language(t)).expect("a voice is installed");
        let backend = Backend::new();
        let cancel = CancelToken::new();
        for (lang, slow) in [(lang, false), (lang, true), (LangId::AUTO, false)] {
            let req = SpeakRequest { text: "Hello there, this is a test.".into(), lang, slow };
            let bytes = fix_wav(backend.speak(&req, &cancel).unwrap()).unwrap();
            assert_eq!(&bytes[..4], b"RIFF");
            assert_eq!(&bytes[8..12], b"WAVE");
            assert!(bytes.len() > 10_000, "{} bytes", bytes.len());
            eprintln!("{} slow={slow}: {} bytes", lang.english_name(), bytes.len());
        }
        // A language with no voice fails with a message, not another voice.
        let missing = LangId::all().find(|l| !tags.iter().any(|t| voice_language(t) == Some(*l)));
        if let Some(missing) = missing {
            let req = SpeakRequest { text: "test".into(), lang: missing, slow: false };
            let err = backend.speak(&req, &cancel).unwrap_err().to_string();
            assert!(err.contains(missing.english_name()), "{err}");
        }
    }

    #[test]
    #[ignore = "speaks through Windows"]
    fn live_windows_voice_honours_cancel() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let req = SpeakRequest { text: "word ".repeat(2000), lang: LangId::AUTO, slow: false };
        assert!(matches!(Backend::new().speak(&req, &cancel), Err(Error::Cancelled)));
    }
}

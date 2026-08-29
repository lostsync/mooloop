//! Audio-file import boundary.
//!
//! Containers and codecs end here: callers receive immutable stereo `f32`
//! frames regardless of the source format. Import runs on UI-owned worker
//! threads, never in the realtime audio callback.

use mooloop_dsp::SampleData;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Extensions offered by the picker and sample browser. Symphonia still
/// probes file contents; this list is only the user-facing discovery policy.
pub(crate) const SUPPORTED_EXTENSIONS: [&str; 7] =
    ["wav", "aif", "aiff", "mp3", "flac", "ogg", "oga"];

pub(crate) struct DecodedAudioFile {
    pub(crate) sample: Arc<SampleData>,
    pub(crate) source_channels: usize,
    pub(crate) bits_per_sample: Option<u32>,
    pub(crate) codec_name: &'static str,
}

pub(crate) fn is_supported_extension(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        SUPPORTED_EXTENSIONS
            .iter()
            .any(|supported| extension.eq_ignore_ascii_case(supported))
    })
}

/// Probe and fully decode one audio file into mooloop's canonical sample
/// representation. Multichannel sources retain their first two channels;
/// mono sources are duplicated to both sides, matching the historical WAV
/// loader's behavior.
pub(crate) fn decode(path: &Path) -> Result<DecodedAudioFile, String> {
    let file = File::open(path).map_err(|error| format!("could not open sample: {error}"))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
        hint.with_extension(extension);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| format!("unsupported or malformed audio file: {error}"))?;

    let (track_id, codec_params) = {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| "file contained no audio track".to_string())?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or_else(|| "audio track had no codec parameters".to_string())?;
        (track.id, params.clone())
    };

    let codec_registry = symphonia::default::get_codecs();
    let codec_name = codec_registry
        .get_audio_decoder(codec_params.codec)
        .map(|decoder| decoder.codec.info.short_name)
        .unwrap_or("unknown");
    let mut decoder = codec_registry
        .make_audio_decoder(&codec_params, &AudioDecoderOptions::default())
        .map_err(|error| format!("unsupported audio codec: {error}"))?;

    let mut frames = Vec::new();
    let mut interleaved = Vec::<f32>::new();
    let mut sample_rate = codec_params.sample_rate;
    let mut source_channels = codec_params
        .channels
        .as_ref()
        .map_or(0, |value| value.count());

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(Error::ResetRequired) => {
                return Err("chained audio streams are not supported".to_string());
            }
            Err(error) => return Err(format!("could not read audio packet: {error}")),
        };
        if packet.track_id != track_id {
            continue;
        }

        let decoded = decoder
            .decode(&packet)
            .map_err(|error| format!("sample decode failed: {error}"))?;
        let packet_rate = decoded.spec().rate();
        let packet_channels = decoded.spec().channels().count();
        if packet_channels == 0 {
            return Err("decoded audio packet had no channels".to_string());
        }
        if sample_rate.is_some_and(|rate| rate != packet_rate) {
            return Err("sample rate changed during audio stream".to_string());
        }
        if source_channels != 0 && source_channels != packet_channels {
            return Err("channel layout changed during audio stream".to_string());
        }
        sample_rate = Some(packet_rate);
        source_channels = packet_channels;

        interleaved.resize(decoded.samples_interleaved(), 0.0);
        decoded.copy_to_slice_interleaved(&mut interleaved);
        frames.extend(interleaved.chunks_exact(packet_channels).map(|frame| {
            let left = frame[0];
            let right = frame.get(1).copied().unwrap_or(left);
            [left, right]
        }));
    }

    if frames.is_empty() {
        return Err("file contained no samples".to_string());
    }

    Ok(DecodedAudioFile {
        sample: Arc::new(SampleData {
            frames,
            sample_rate: sample_rate.ok_or_else(|| "sample rate was missing".to_string())?,
            root_note: 60,
        }),
        source_channels,
        bits_per_sample: codec_params.bits_per_sample,
        codec_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_content_instead_of_trusting_the_extension() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("misnamed.mp3");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        writer.write_sample(16_384_i16).unwrap();
        writer.finalize().unwrap();

        let decoded = decode(&path).unwrap();
        assert_eq!(decoded.source_channels, 1);
        assert_eq!(decoded.sample.frames, vec![[0.5, 0.5]]);
    }

    #[test]
    fn user_facing_extensions_cover_enabled_formats() {
        for extension in ["wav", "aif", "aiff", "mp3", "flac", "ogg", "oga"] {
            assert!(is_supported_extension(Path::new(&format!(
                "sample.{extension}"
            ))));
            assert!(is_supported_extension(Path::new(&format!(
                "sample.{}",
                extension.to_uppercase()
            ))));
        }
        assert!(!is_supported_extension(Path::new("sample.opus")));
        assert!(!is_supported_extension(Path::new("sample.txt")));
    }

    /// Manual codec-matrix check. `ffmpeg` is only a test-fixture generator;
    /// production import has no native or process dependency.
    #[test]
    #[ignore = "requires ffmpeg"]
    fn decodes_enabled_formats_with_ffmpeg() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&source, spec).unwrap();
        for frame in 0..4_410_i32 {
            let value = ((frame % 200) * 160 - 16_000) as i16;
            writer.write_sample(value).unwrap();
            writer.write_sample(-value).unwrap();
        }
        writer.finalize().unwrap();

        for (extension, expected_codec) in [
            ("aiff", "pcm"),
            ("mp3", "mp3"),
            ("flac", "flac"),
            ("ogg", "vorbis"),
        ] {
            let output = temp.path().join(format!("sample.{extension}"));
            let status = std::process::Command::new("ffmpeg")
                .args(["-loglevel", "error", "-y", "-i"])
                .arg(&source)
                .arg(&output)
                .status()
                .expect("ffmpeg must be installed for this ignored test");
            assert!(status.success(), "could not generate {extension} fixture");

            let decoded = decode(&output).unwrap();
            assert_eq!(decoded.sample.sample_rate, 44_100);
            assert!(!decoded.sample.frames.is_empty());
            assert!(
                decoded.codec_name.contains(expected_codec),
                "{extension} selected unexpected codec {}",
                decoded.codec_name
            );
        }
    }
}

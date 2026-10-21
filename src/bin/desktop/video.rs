//! Embedded video player: FFmpeg demuxes and decodes, cpal plays the audio and egui shows the
//! frames as a texture. The audio output drives the clock (wall time when there is no audio);
//! a video frame is shown once the clock reaches its timestamp.
//!
//! Threads: one demuxer (also decodes subtitles), one video decoder, one audio decoder. A seek
//! bumps `epoch`; anything tagged with an older epoch is dropped wherever it is found.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use ffmpeg_next as ff;
use ff::ChannelLayout;
use ff::codec::threading;
use ff::format::Sample;
use ff::format::sample::Type as SampleType;
use ff::media::Type as Media;
use ff::software::{resampling, scaling};

/// Frames are scaled down to fit this box before upload.
const MAX_W: u32 = 1920;
const MAX_H: u32 = 1080;
/// Converted frames waiting for their time.
const FRAME_QUEUE: usize = 4;
/// Decoded audio kept ahead of the output, in seconds.
const AUDIO_AHEAD: f64 = 1.0;
/// Demuxing pauses once every active packet queue holds this many packets.
const PACKET_QUEUE: usize = 150;
/// ...or once any single queue holds this many (sparse interleaving).
const PACKET_QUEUE_MAX: usize = 3000;

pub struct Track {
    pub index: usize,
    pub label: String,
}

pub struct Info {
    /// Seconds, when the container knows it.
    pub duration: Option<f64>,
    pub audio_tracks: Vec<Track>,
    pub sub_tracks: Vec<Track>,
    pub audio: Option<usize>,
    pub sub: Option<usize>,
}

#[derive(Clone, Copy)]
struct Clock {
    pts: f64,
    at: Instant,
    running: bool,
}

impl Clock {
    fn now(&self) -> f64 {
        if !self.running {
            return self.pts;
        }
        let now = Instant::now();
        if now >= self.at {
            self.pts + (now - self.at).as_secs_f64()
        } else {
            self.pts - (self.at - now).as_secs_f64()
        }
    }
}

struct VFrame {
    epoch: u64,
    pts: f64,
    image: ColorImage,
}

struct Chunk {
    epoch: u64,
    pts: f64,
    data: Vec<f32>,
    pos: usize,
}

#[derive(Default)]
struct AudioBuf {
    chunks: VecDeque<Chunk>,
    /// Samples (all channels) not yet played.
    queued: usize,
}

struct Sub {
    start: f64,
    end: f64,
    text: String,
}

struct Shared {
    ctx: egui::Context,
    stop: AtomicBool,
    epoch: AtomicU64,
    /// The epoch the demuxer has seeked to; differs from `epoch` while a seek is pending.
    demux_epoch: AtomicU64,
    paused: AtomicBool,
    volume: AtomicU32,
    /// Set once the first frame after a start or seek is queued; audio waits for it.
    prerolled: AtomicBool,
    has_audio: AtomicBool,
    audio_eof: AtomicBool,
    video_eof: AtomicBool,
    vq: AtomicUsize,
    aq: AtomicUsize,
    clock: Mutex<Clock>,
    frames: Mutex<VecDeque<VFrame>>,
    audio: Mutex<AudioBuf>,
    subs: Mutex<Vec<Sub>>,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Relaxed)
    }

    fn clock_now(&self) -> f64 {
        self.clock.lock().unwrap().now()
    }

    /// The cpal callback: plays queued audio and moves the clock to what is being heard.
    fn fill_audio(&self, out: &mut [f32], info: &cpal::OutputCallbackInfo, rate: u32, channels: usize) {
        let ts = info.timestamp();
        let latency = ts.playback.saturating_duration_since(ts.callback);
        let epoch = self.epoch.load(Acquire);
        let play = !self.paused.load(Relaxed) && self.prerolled.load(Acquire);
        let mut written = 0;
        let mut first_pts = None;
        {
            let mut buf = self.audio.lock().unwrap();
            while buf.chunks.front().is_some_and(|c| c.epoch != epoch) {
                let c = buf.chunks.pop_front().unwrap();
                buf.queued -= c.data.len() - c.pos;
            }
            if play {
                let vol = f32::from_bits(self.volume.load(Relaxed));
                while written < out.len() {
                    let Some(c) = buf.chunks.front_mut() else { break };
                    if first_pts.is_none() {
                        first_pts = Some(c.pts + (c.pos / channels) as f64 / rate as f64);
                    }
                    let n = (out.len() - written).min(c.data.len() - c.pos);
                    for (o, s) in out[written..written + n].iter_mut().zip(&c.data[c.pos..c.pos + n]) {
                        *o = s * vol;
                    }
                    written += n;
                    c.pos += n;
                    if c.pos == c.data.len() {
                        buf.chunks.pop_front();
                    }
                }
                buf.queued -= written;
            }
        }
        out[written..].fill(0.0);

        let mut clock = self.clock.lock().unwrap();
        if self.epoch.load(Acquire) != epoch {
            return; // a seek happened meanwhile and already reset the clock
        }
        let now = Instant::now();
        *clock = match first_pts {
            Some(pts) => Clock { pts, at: now + latency, running: true },
            // Audio ended before the video: keep the clock going on wall time.
            None if play && self.audio_eof.load(Acquire) => {
                if clock.running {
                    return;
                }
                Clock { pts: clock.pts, at: now, running: true }
            }
            None => Clock { pts: clock.now(), at: now, running: false },
        };
    }
}

enum Cmd {
    Seek(f64, u64),
    Audio(usize),
    Sub(Option<usize>),
}

enum Pkt<D> {
    Packet(ff::Packet, u64),
    Flush { epoch: u64, target: f64 },
    /// Replaces the decoder (audio track switch).
    Decoder(D),
    Eof,
}

type AudioDecoder = (ff::decoder::Audio, f64);

pub struct Player {
    shared: Arc<Shared>,
    cmd: Sender<Cmd>,
    info_rx: Receiver<Result<Info, String>>,
    pub info: Option<Result<Info, String>>,
    texture: Option<TextureHandle>,
    /// Whether a frame of the current epoch is on screen.
    has_frame: bool,
    _audio_out: Option<cpal::Stream>,
}

impl Player {
    pub fn open(url: String, ctx: &egui::Context) -> Player {
        let shared = Arc::new(Shared {
            ctx: ctx.clone(),
            stop: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            demux_epoch: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            prerolled: AtomicBool::new(false),
            has_audio: AtomicBool::new(false),
            audio_eof: AtomicBool::new(false),
            video_eof: AtomicBool::new(false),
            vq: AtomicUsize::new(0),
            aq: AtomicUsize::new(0),
            clock: Mutex::new(Clock { pts: 0.0, at: Instant::now(), running: false }),
            frames: Mutex::new(VecDeque::new()),
            audio: Mutex::new(AudioBuf::default()),
            subs: Mutex::new(Vec::new()),
        });
        let audio_out = match start_audio(shared.clone()) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!("no audio output: {e:#}");
                None
            }
        };
        let out_format = audio_out.as_ref().map(|(_, rate, ch)| (*rate, *ch));
        let (cmd, cmd_rx) = mpsc::channel();
        let (info_tx, info_rx) = mpsc::channel();
        let sh = shared.clone();
        thread::Builder::new()
            .name("demux".into())
            .spawn(move || {
                if let Err(e) = demux(&url, sh.clone(), cmd_rx, &info_tx, out_format) {
                    tracing::warn!("playback failed: {e:#}");
                    let _ = info_tx.send(Err(format!("{e:#}")));
                }
                sh.ctx.request_repaint();
            })
            .expect("failed to spawn the demux thread");
        Player {
            shared,
            cmd,
            info_rx,
            info: None,
            texture: None,
            has_frame: false,
            _audio_out: audio_out.map(|(s, ..)| s),
        }
    }

    pub fn duration(&self) -> Option<f64> {
        self.info.as_ref().and_then(|i| i.as_ref().ok()).and_then(|i| i.duration)
    }

    pub fn position(&self) -> f64 {
        let t = self.shared.clock_now().max(0.0);
        self.duration().map_or(t, |d| t.min(d))
    }

    pub fn paused(&self) -> bool {
        self.shared.paused.load(Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Relaxed);
        self.shared.ctx.request_repaint();
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Relaxed))
    }

    pub fn set_volume(&self, v: f32) {
        self.shared.volume.store(v.clamp(0.0, 1.0).to_bits(), Relaxed);
    }

    pub fn ended(&self) -> bool {
        let s = &self.shared;
        s.video_eof.load(Acquire)
            && s.frames.lock().unwrap().is_empty()
            && (!s.has_audio.load(Acquire) || (s.audio_eof.load(Acquire) && s.audio.lock().unwrap().queued == 0))
    }

    /// Playing but the clock is stalled: waiting for data.
    pub fn buffering(&self) -> bool {
        matches!(self.info, Some(Ok(_)))
            && !self.paused()
            && !self.ended()
            && !self.shared.clock.lock().unwrap().running
    }

    pub fn seek(&mut self, t: f64) {
        let t = self.duration().map_or(t, |d| t.min(d - 0.5)).max(0.0);
        let s = &self.shared;
        let epoch = s.epoch.fetch_add(1, AcqRel) + 1;
        s.prerolled.store(false, Release);
        *s.audio.lock().unwrap() = AudioBuf::default();
        s.frames.lock().unwrap().clear();
        *s.clock.lock().unwrap() = Clock { pts: t, at: Instant::now(), running: false };
        self.has_frame = false;
        let _ = self.cmd.send(Cmd::Seek(t, epoch));
    }

    pub fn set_audio_track(&mut self, index: usize) {
        if let Some(Ok(info)) = &mut self.info {
            info.audio = Some(index);
        }
        let _ = self.cmd.send(Cmd::Audio(index));
        self.seek(self.position());
    }

    pub fn set_sub_track(&mut self, index: Option<usize>) {
        if let Some(Ok(info)) = &mut self.info {
            info.sub = index;
        }
        let _ = self.cmd.send(Cmd::Sub(index));
        self.seek(self.position());
    }

    pub fn subtitle(&self) -> Option<String> {
        let t = self.position();
        let subs = self.shared.subs.lock().unwrap();
        let lines: Vec<&str> = subs.iter().filter(|s| s.start <= t && t < s.end).map(|s| s.text.as_str()).collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// Advances playback; call every UI frame. Returns the frame to draw.
    pub fn update(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if let Ok(info) = self.info_rx.try_recv() {
            self.info = Some(info);
        }
        let s = &self.shared;
        if !s.has_audio.load(Acquire) {
            // Wall clock: runs while frames keep coming.
            let starved = s.frames.lock().unwrap().is_empty() && !s.video_eof.load(Acquire);
            let run = !self.paused() && s.prerolled.load(Acquire) && !starved;
            let mut c = s.clock.lock().unwrap();
            if c.running != run {
                *c = Clock { pts: c.now(), at: Instant::now(), running: run };
            }
        }

        let clock = s.clock_now();
        let epoch = s.epoch.load(Acquire);
        let mut show = None;
        let mut next = None;
        {
            let mut q = s.frames.lock().unwrap();
            q.retain(|f| f.epoch == epoch);
            while let Some(f) = q.front() {
                if f.pts <= clock + 0.005 || !self.has_frame {
                    show = q.pop_front();
                    self.has_frame = true;
                } else {
                    next = Some(f.pts);
                    break;
                }
            }
        }
        if let Some(f) = show {
            match &mut self.texture {
                Some(t) => t.set(f.image, TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("video", f.image, TextureOptions::LINEAR)),
            }
        }
        if !self.paused() {
            let wait = next.map_or(0.02, |n| (n - clock).clamp(0.002, 0.05));
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }
        self.texture.as_ref()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // The demuxer's interrupt callback sees this and aborts any blocking read.
        self.shared.stop.store(true, Relaxed);
    }
}

fn start_audio(shared: Arc<Shared>) -> Result<(cpal::Stream, u32, u16)> {
    let device = cpal::default_host().default_output_device().context("no audio output device")?;
    let config = device.default_output_config()?.config();
    let (rate, channels) = (config.sample_rate, config.channels);
    let stream = device.build_output_stream(
        config,
        move |out: &mut [f32], info: &cpal::OutputCallbackInfo| {
            shared.fill_audio(out, info, rate, channels as usize)
        },
        |e| tracing::warn!("audio output error: {e}"),
        None,
    )?;
    stream.play()?;
    Ok((stream, rate, channels))
}

fn track_label(s: &ff::Stream, n: usize) -> String {
    let md = s.metadata();
    let mut parts = Vec::new();
    if let Some(l) = md.get("language").filter(|l| *l != "und") {
        parts.push(l.to_uppercase());
    }
    if let Some(t) = md.get("title") {
        parts.push(t.to_string());
    }
    parts.push(format!("{:?}", s.parameters().id()).to_lowercase());
    format!("{n}. {}", parts.join(" · "))
}

fn decoder_for(s: &ff::Stream) -> Result<ff::decoder::Decoder> {
    let mut ctx = ff::codec::Context::from_parameters(s.parameters())?;
    ctx.set_threading(threading::Config::kind(threading::Type::Frame));
    let mut dec = ctx.decoder();
    dec.set_packet_time_base(s.time_base());
    Ok(dec)
}

fn open_audio(s: &ff::Stream) -> Result<AudioDecoder> {
    Ok((decoder_for(s)?.audio()?, f64::from(s.time_base())))
}

fn open_sub(s: &ff::Stream) -> Option<(ff::decoder::Subtitle, f64)> {
    let dec = decoder_for(s).and_then(|d| Ok(d.subtitle()?));
    match dec {
        Ok(d) => Some((d, f64::from(s.time_base()))),
        Err(e) => {
            tracing::warn!("subtitle track {}: {e:#}", s.index());
            None
        }
    }
}

/// Bitmap subtitles (PGS, DVD, DVB) would need compositing; only text formats are offered.
fn is_text_sub(id: ff::codec::Id) -> bool {
    use ff::codec::Id;
    !matches!(id, Id::HDMV_PGS_SUBTITLE | Id::DVD_SUBTITLE | Id::DVB_SUBTITLE | Id::XSUB)
}

fn demux(
    url: &str,
    shared: Arc<Shared>,
    cmds: Receiver<Cmd>,
    info_tx: &Sender<Result<Info, String>>,
    audio_out: Option<(u32, u16)>,
) -> Result<()> {
    ff::init()?;
    ff::log::set_level(ff::log::Level::Error);
    let sh = shared.clone();
    let mut input = ff::format::input_with_interrupt(url, move || {
        sh.stopped() || sh.epoch.load(Acquire) != sh.demux_epoch.load(Acquire)
    })
    .context("could not open the stream")?;

    let video = input.streams().best(Media::Video).map(|s| s.index());
    let mut audio = audio_out.and(input.streams().best(Media::Audio).map(|s| s.index()));
    let mut info = Info {
        duration: (input.duration() > 0).then(|| input.duration() as f64 / f64::from(ff::ffi::AV_TIME_BASE)),
        audio_tracks: Vec::new(),
        sub_tracks: Vec::new(),
        audio,
        sub: None,
    };
    for s in input.streams() {
        let params = s.parameters();
        match params.medium() {
            Media::Audio => info.audio_tracks.push(Track { index: s.index(), label: track_label(&s, info.audio_tracks.len() + 1) }),
            Media::Subtitle if is_text_sub(params.id()) => {
                let lang = s.metadata().get("language").unwrap_or_default().to_lowercase();
                let preferred = ["rum", "ron", "rom", "ro"].contains(&lang.as_str())
                    || s.disposition().contains(ff::format::stream::Disposition::FORCED);
                if preferred && info.sub.is_none() {
                    info.sub = Some(s.index());
                }
                info.sub_tracks.push(Track { index: s.index(), label: track_label(&s, info.sub_tracks.len() + 1) });
            }
            _ => {}
        }
    }

    let (vtx, vrx) = mpsc::channel::<Pkt<()>>();
    let (atx, arx) = mpsc::channel::<Pkt<AudioDecoder>>();
    if let Some(i) = video {
        let s = input.stream(i).unwrap();
        let dec = decoder_for(&s)?.video().context("unsupported video codec")?;
        let tb = f64::from(s.time_base());
        let sh = shared.clone();
        thread::Builder::new().name("video".into()).spawn(move || video_thread(sh, vrx, dec, tb))?;
    } else {
        shared.prerolled.store(true, Release);
        shared.video_eof.store(true, Release);
    }
    let mut audio_dec = None;
    if let Some(i) = audio {
        match open_audio(&input.stream(i).unwrap()) {
            Ok(d) => audio_dec = Some(d),
            Err(e) => {
                tracing::warn!("audio track {i}: {e:#}");
                audio = None;
                info.audio = None;
            }
        }
    }
    if let (Some(dec), Some((rate, channels))) = (audio_dec, audio_out) {
        shared.has_audio.store(true, Release);
        let sh = shared.clone();
        thread::Builder::new().name("audio".into()).spawn(move || audio_thread(sh, arx, dec, rate, channels))?;
    }
    let mut sub = info.sub.and_then(|i| open_sub(&input.stream(i).unwrap()));
    let mut sub_idx = info.sub;
    let _ = info_tx.send(Ok(info));
    shared.ctx.request_repaint();

    let mut epoch = 0;
    let mut eof = false;
    while !shared.stopped() {
        while let Ok(cmd) = cmds.try_recv() {
            match cmd {
                Cmd::Seek(t, e) => {
                    // Before seeking, so the interrupt callback lets the seek's own reads through.
                    shared.demux_epoch.store(e, Release);
                    let ts = (t * f64::from(ff::ffi::AV_TIME_BASE)) as i64;
                    if let Err(err) = input.seek(ts, ..ts) {
                        tracing::warn!("seek to {t:.1}s failed: {err}");
                    }
                    epoch = e;
                    eof = false;
                    let _ = vtx.send(Pkt::Flush { epoch: e, target: t });
                    let _ = atx.send(Pkt::Flush { epoch: e, target: t });
                }
                Cmd::Audio(i) => match input.stream(i).map(|s| open_audio(&s)) {
                    Some(Ok(dec)) => {
                        audio = Some(i);
                        let _ = atx.send(Pkt::Decoder(dec));
                    }
                    Some(Err(e)) => tracing::warn!("audio track {i}: {e:#}"),
                    None => {}
                },
                Cmd::Sub(i) => {
                    sub_idx = i;
                    sub = i.and_then(|i| input.stream(i)).and_then(|s| open_sub(&s));
                    shared.subs.lock().unwrap().clear();
                }
            }
        }
        if shared.epoch.load(Acquire) != epoch {
            // The seek command is on its way.
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let full = |q: &AtomicUsize, active: bool| !active || q.load(Relaxed) >= PACKET_QUEUE;
        let (vq, aq) = (shared.vq.load(Relaxed), shared.aq.load(Relaxed));
        let throttled = (full(&shared.vq, video.is_some()) && full(&shared.aq, audio.is_some()))
            || vq >= PACKET_QUEUE_MAX
            || aq >= PACKET_QUEUE_MAX;
        if eof || throttled {
            thread::sleep(Duration::from_millis(10));
            continue;
        }

        let mut packet = ff::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {
                let i = packet.stream();
                if Some(i) == video {
                    shared.vq.fetch_add(1, Relaxed);
                    let _ = vtx.send(Pkt::Packet(packet, epoch));
                } else if Some(i) == audio {
                    shared.aq.fetch_add(1, Relaxed);
                    let _ = atx.send(Pkt::Packet(packet, epoch));
                } else if Some(i) == sub_idx
                    && let Some((dec, tb)) = &mut sub
                {
                    decode_sub(&shared, dec, *tb, &packet);
                }
            }
            Err(ff::Error::Eof) => {
                eof = true;
                let _ = vtx.send(Pkt::Eof);
                let _ = atx.send(Pkt::Eof);
            }
            // Interrupted for a seek (handled next iteration) or stop.
            Err(ff::Error::Exit) if shared.stopped() || shared.epoch.load(Acquire) != epoch => {}
            Err(ff::Error::Other { errno }) if errno == ff::util::error::EAGAIN => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                tracing::warn!("read error: {e}");
                eof = true;
                let _ = vtx.send(Pkt::Eof);
                let _ = atx.send(Pkt::Eof);
            }
        }
    }
    Ok(())
}

fn decode_sub(shared: &Shared, dec: &mut ff::decoder::Subtitle, tb: f64, packet: &ff::Packet) {
    let mut sub = ff::Subtitle::new();
    if !matches!(dec.decode(packet, &mut sub), Ok(true)) {
        return;
    }
    let base = packet.pts().unwrap_or(0) as f64 * tb;
    let start = base + sub.start() as f64 / 1000.0;
    let end = if sub.end() > sub.start() {
        base + sub.end() as f64 / 1000.0
    } else {
        base + packet.duration().max(0) as f64 * tb
    };
    let text: Vec<String> = sub
        .rects()
        .filter_map(|r| match r {
            ff::subtitle::Rect::Text(t) => Some(t.get().trim().to_string()),
            ff::subtitle::Rect::Ass(a) => Some(ass_text(a.get())),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect();
    if text.is_empty() {
        return;
    }
    let text = text.join("\n");
    let mut subs = shared.subs.lock().unwrap();
    if !subs.iter().any(|s| s.start == start && s.text == text) {
        subs.push(Sub { start, end, text });
    }
}

/// Text of an ASS event (`ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`)
/// without override tags.
fn ass_text(event: &str) -> String {
    let raw = event.splitn(9, ',').nth(8).unwrap_or(event);
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for c in raw.chars() {
        match c {
            '{' => in_tag = true,
            '}' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("\\N", "\n").replace("\\n", "\n").replace("\\h", " ").trim().to_string()
}

fn video_thread(shared: Arc<Shared>, rx: Receiver<Pkt<()>>, mut dec: ff::decoder::Video, tb: f64) {
    let sar = dec.aspect_ratio();
    let sar = if sar.numerator() > 0 && sar.denominator() > 0 { f64::from(sar) } else { 1.0 };
    let mut frame = ff::frame::Video::empty();
    let mut scaler: Option<(scaling::Context, ff::frame::Video)> = None;
    let mut epoch = 0;
    let mut target = 0.0;
    let mut eof = false;
    let mut last_pts = 0.0;
    while !shared.stopped() {
        let msg = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match msg {
            Pkt::Packet(p, e) => {
                shared.vq.fetch_sub(1, Relaxed);
                if e != shared.epoch.load(Acquire) || dec.send_packet(&p).is_err() {
                    continue;
                }
            }
            Pkt::Flush { epoch: e, target: t } => {
                dec.flush();
                (epoch, target, eof) = (e, t, false);
                shared.video_eof.store(false, Release);
                continue;
            }
            Pkt::Eof => {
                let _ = dec.send_eof();
                eof = true;
            }
            Pkt::Decoder(()) => continue,
        }
        'frames: while dec.receive_frame(&mut frame).is_ok() {
            let pts = frame.timestamp().map_or(last_pts, |t| t as f64 * tb);
            last_pts = pts;
            if pts < target - 0.01 {
                continue; // between the keyframe and the seek target
            }
            if shared.prerolled.load(Acquire) && pts < shared.clock_now() - 0.25 {
                continue; // too late to show; skip the conversion
            }
            let Some(image) = to_image(&frame, sar, &mut scaler) else { continue };
            let mut image = Some(image);
            loop {
                if shared.stopped() || shared.epoch.load(Acquire) != epoch {
                    break 'frames;
                }
                {
                    let mut q = shared.frames.lock().unwrap();
                    if q.len() < FRAME_QUEUE {
                        q.push_back(VFrame { epoch, pts, image: image.take().unwrap() });
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(3));
            }
            shared.prerolled.store(true, Release);
            shared.ctx.request_repaint();
        }
        if eof {
            shared.video_eof.store(true, Release);
            shared.prerolled.store(true, Release);
            shared.ctx.request_repaint();
        }
    }
}

/// Scales (keeping the display aspect, within MAX_W x MAX_H) and converts to RGBA.
fn to_image(
    frame: &ff::frame::Video,
    sar: f64,
    scaler: &mut Option<(scaling::Context, ff::frame::Video)>,
) -> Option<ColorImage> {
    let (w, h) = (frame.width(), frame.height());
    if w == 0 || h == 0 {
        return None;
    }
    let disp_w = w as f64 * sar;
    let scale = (MAX_W as f64 / disp_w).min(MAX_H as f64 / h as f64).min(1.0);
    let dw = ((disp_w * scale) as u32 & !1).max(2);
    let dh = ((h as f64 * scale) as u32 & !1).max(2);
    let stale = scaler.as_ref().is_none_or(|(s, _)| {
        let (i, o) = (s.input(), s.output());
        i.format != frame.format() || i.width != w || i.height != h || o.width != dw || o.height != dh
    });
    if stale {
        let ctx = scaling::Context::get(frame.format(), w, h, ff::format::Pixel::RGBA, dw, dh, scaling::Flags::BILINEAR)
            .map_err(|e| tracing::warn!("scaler: {e}"))
            .ok()?;
        *scaler = Some((ctx, ff::frame::Video::empty()));
    }
    let (ctx, out) = scaler.as_mut().unwrap();
    ctx.run(frame, out).map_err(|e| tracing::warn!("scale: {e}")).ok()?;
    let (stride, data) = (out.stride(0), out.data(0));
    let (dw, dh) = (dw as usize, dh as usize);
    let mut pixels = Vec::with_capacity(dw * dh);
    for row in data.chunks(stride).take(dh) {
        pixels.extend(row[..dw * 4].as_chunks::<4>().0.iter().map(|p| Color32::from_rgb(p[0], p[1], p[2])));
    }
    Some(ColorImage::new([dw, dh], pixels))
}

fn audio_thread(shared: Arc<Shared>, rx: Receiver<Pkt<AudioDecoder>>, dec: AudioDecoder, rate: u32, channels: u16) {
    let (mut dec, mut tb) = dec;
    let out_layout = ChannelLayout::default(channels as i32);
    let out_format = Sample::F32(SampleType::Packed);
    let mut frame = ff::frame::Audio::empty();
    let mut resampler: Option<(resampling::Context, Sample, ChannelLayout, u32)> = None;
    let mut epoch = 0;
    let mut target = 0.0;
    let mut eof = false;
    let mut next_pts = 0.0;
    while !shared.stopped() {
        let msg = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match msg {
            Pkt::Packet(p, e) => {
                shared.aq.fetch_sub(1, Relaxed);
                if e != shared.epoch.load(Acquire) || dec.send_packet(&p).is_err() {
                    continue;
                }
            }
            Pkt::Flush { epoch: e, target: t } => {
                dec.flush();
                (epoch, target, eof, next_pts) = (e, t, false, t);
                resampler = None;
                shared.audio_eof.store(false, Release);
                continue;
            }
            Pkt::Decoder((d, t)) => {
                (dec, tb) = (d, t);
                resampler = None;
                continue;
            }
            Pkt::Eof => {
                let _ = dec.send_eof();
                eof = true;
            }
        }
        'frames: while dec.receive_frame(&mut frame).is_ok() {
            if frame.rate() == 0 || frame.samples() == 0 {
                continue;
            }
            let pts = frame.timestamp().map_or(next_pts, |t| t as f64 * tb);
            next_pts = pts + frame.samples() as f64 / frame.rate() as f64;
            if next_pts < target {
                continue;
            }
            let (fmt, layout, in_rate) = (frame.format(), frame.channel_layout(), frame.rate());
            let stale = resampler.as_ref().is_none_or(|(_, f, l, r)| *f != fmt || *l != layout || *r != in_rate);
            if stale {
                match resampling::Context::get(fmt, layout, in_rate, out_format, out_layout, rate) {
                    Ok(r) => resampler = Some((r, fmt, layout, in_rate)),
                    Err(e) => {
                        tracing::warn!("resampler: {e}");
                        continue;
                    }
                }
            }
            let (r, ..) = resampler.as_mut().unwrap();
            let cap = frame.samples() * rate as usize / in_rate as usize + 256;
            let mut out = ff::frame::Audio::new(out_format, cap, out_layout);
            if let Err(e) = r.run(&frame, &mut out) {
                tracing::warn!("resample: {e}");
                continue;
            }
            let n = out.samples() * channels as usize;
            let data: Vec<f32> = out.data(0)[..n * 4]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_ne_bytes(*b))
                .collect();
            loop {
                if shared.stopped() || shared.epoch.load(Acquire) != epoch {
                    break 'frames;
                }
                let mut buf = shared.audio.lock().unwrap();
                let ahead = buf.queued as f64 / (rate as f64 * channels as f64);
                if ahead < AUDIO_AHEAD {
                    buf.queued += data.len();
                    buf.chunks.push_back(Chunk { epoch, pts, data, pos: 0 });
                    break;
                }
                drop(buf);
                thread::sleep(Duration::from_millis(10));
            }
        }
        if eof {
            shared.audio_eof.store(true, Release);
        }
    }
}


# Takery

**Takery** is the video editor of the [Arch Quilly](https://github.com/Sapyst/quilly) suite: a native non-linear editor (Rust, wgpu/Vulkan) with a timeline, trimming and ripple edits, color, audio mixing and effects, titles and captions, and exports to H.264, HEVC, ProRes, DNxHR and more.

It is a fork of **[FilmCraft](https://github.com/storytold/filmcraft) by the ArtCraft team** (MIT OR Apache-2.0), which provides the engine. Takery adds:

- **Auto Captions** (Sequence › Captions › Auto Captions…, `Ctrl+Alt+Shift+C`): one click transcribes the sequence's speech and lays it out as captions on a new caption track, in the background with a progress card and Cancel (`crates/engine/src/auto_captions.rs`, `crates/ui-egui/src/auto_captions.rs`). The language is detected or chosen.
- **Whisper on the GPU** through whisper.cpp and Vulkan (`crates/speech/src/whisper_cpp.rs`, needs `whisper-cli`): the `gpu-whisper-*` models, large-v3 turbo by default, pinned to ggerganov/whisper.cpp and checksum-checked; 14 s of speech take about 1 s on an RTX 5070.
- **Hardware acceleration by default**: VA-API / NVDEC decoding, NVENC H.264 encoding and the GPU export compositor, each with the software path as fallback.
- the **Aero** look (`crates/ui-egui/src/aero.rs`) and the suite's name, icon and links (`com.archquilly.Takery`, data in `~/.local/share/arch-quilly/takery`).

The ArtCraft name and marks are not used: as `docs/brand/LICENSE-brand.txt` asks of modified versions, the mark files were removed from this branch and the app shows the suite's icon instead. FilmCraft's own README is kept in [`docs/upstream-README.md`](docs/upstream-README.md).

## Build

```sh
git clone https://github.com/storytold/craft-fonts ../craft-fonts
CRAFT_FONTS_DIR="$PWD/../craft-fonts" cargo build --release -p filmcraft --features whisper
```

On Arch Linux, install the suite's package instead (`arch-takery` or the whole `arch-quilly`, from the [Arch Quilly releases](https://github.com/Sapyst/quilly/releases)); the Quilly hub updates it. The GPU captions need `whisper-cpp` and `ggml-vulkan`.

Branch `takery` follows FilmCraft's `main` (`git fetch upstream && git rebase upstream/main`).

## License

MIT OR Apache-2.0, as FilmCraft (`LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `ATTRIBUTION.md`).

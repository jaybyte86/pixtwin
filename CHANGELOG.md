# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [0.1.0] – 2026-11-07

### Added
- Similar-image search based on a 50×50 colour signature with coarse and fine comparison.
- Detects resized, recompressed, slightly cropped, brightened and 90°-rotated copies.
- Match level slider (strict … loose) plus advanced thresholds in Settings.
- Options: rotation search, background weighting, compare only across folders.
- Group view with thumbnails, path, resolution, file size and modification date.
- Auto-mark rules: keep highest resolution, keep largest file, mark longest paths, mark older files, mark newer files.
- Move marked files to the system trash (with confirmation).
- Keyboard shortcuts: Space toggles the mark, Delete opens the trash dialog.
- Headless mode: `pixtwin --cli`.

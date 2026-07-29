# FFmpeg dependency

The source repository intentionally does not track `ffmpeg.exe` or `ffprobe.exe`.

For development, make both commands available through `PATH`, set `FFMPEG_EXE` and `FFPROBE_EXE`, or place them under:

```text
tools/ffmpeg/bin/
```

The application searches those locations in that order.

The checked-in `LICENSE.txt` and `SOURCE.txt` describe the reference build previously used by the maintainers. If a release bundles a different build, update both files and provide the exact corresponding source and build information with that release.

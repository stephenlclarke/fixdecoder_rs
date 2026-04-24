# Icon Assets

`marvin.png` is the canonical source image for application icon assets in this repo.

- `marvin.ico` is embedded into Windows builds of `fixdecoder.exe` and `pcap2fix.exe`.
- `marvin.icns` is kept for future macOS app-bundle packaging.
- `marvin.png` is kept for Linux desktop packaging or launcher metadata.

To regenerate the derived icon files:

```bash
make icons
```

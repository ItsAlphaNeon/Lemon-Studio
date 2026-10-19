# ASIO® SDK (host files)

<img alt="ASIO Compatible logo" src="logo/ASIO-compatible-logo-Steinberg-TM-BW.png" width="96">

The files an ASIO host needs from Steinberg's ASIO SDK, which asio-sys compiles into Lemon Studio's Windows build
(ASIO output, `src/device/asio.rs`). `.cargo/config.toml` points asio-sys here (`CPAL_ASIO_DIR`), so the build does
not download the SDK.

- **Source:** `ASIO-SDK_2.3.4_2025-10-15.zip` from https://www.steinberg.net/asiosdk (8 910 208 bytes, SHA-256
  `d5ebf0c20dd2c5f43771fd0c1418f4b361bf52434ee670097cfa6b3a335e2eca`).
- **License:** used under the GPL version 3 option of the SDK's dual license (`LICENSE.txt`, which applies to the
  files that refer to it). The files are unchanged.
- **Kept:** `common/` (`asio.cpp`, `asio.h`, `asiodrvr.h`, `asiosys.h`, `iasiodrv.h`), `host/` (`asiodrivers.cpp`,
  `asiodrivers.h`, `ginclude.h`) and `host/pc/` (`asiolist.cpp`, `asiolist.h`), with the two copies of `LICENSE.txt`.
- **Logo:** `logo/ASIO-compatible-logo-Steinberg-TM-BW.png` from the SDK's `Steinberg ASIO Logo Artwork/`, unchanged
  (SHA-256 `5b5c4aae4baeaed70da70966713e1dbd255977649f37df4c503752f2207a5c86`). It is Steinberg's trademark artwork,
  not covered by the GPL. `Steinberg ASIO Usage Guidelines.pdf` asks a program that names ASIO to show it in its
  About box, in every dialog that switches ASIO on, and in its documentation and web pages: the Windows build shows it
  in Settings > Audio and Settings > About, and the README and `docs/status.md` show it.
- **Left out:** everything else, which a host does not build: the sample driver (`driver/`) and host (`host/sample/`),
  `host/ASIOConvertSamples.*`, the driver-side files in `common/` (`asiodrvr.cpp`, `combase.*`, `debugmessage.cpp`,
  `dllentry.cpp`, `register.cpp`, `wxdebug.h`), the project files, the PDFs and the other logo files. `combase.*`,
  `dllentry.cpp` and `wxdebug.h` carry Microsoft's own notice instead of the SDK's license.

ASIO is a registered trademark of Steinberg Media Technologies GmbH.

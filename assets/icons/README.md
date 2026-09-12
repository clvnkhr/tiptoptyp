# Application icon

`tiptoptyp.svg` is the canonical mark. The PNG files are deterministic raster
sizes used by eframe and Linux packages; `tiptoptyp.ico` is the Windows icon.
Cargo Packager builds `tiptoptyp.icns` from the PNG set when producing a macOS
bundle, so an independently generated ICNS file is intentionally not checked
in. The 1024-pixel raster keeps the `512@2x` suffix because Cargo Packager uses
that suffix to assign the correct Retina density in the ICNS container.

The artwork mirrors the interface wordmark: two foreground lowercase `t`
shapes followed by one accent-colored `t`. Keep that semantic relationship if
the geometry changes.

# Application icon

`tiptoptyp.svg` is the canonical development-build mark, including its amber
`DEV` badge. The PNG files are deterministic raster
sizes used by eframe and Linux packages; `tiptoptyp.ico` is the Windows icon.
Cargo Packager builds `tiptoptyp.icns` from the PNG set when producing a macOS
bundle, so an independently generated ICNS file is intentionally not checked
in. The 1024-pixel raster keeps the `512@2x` suffix because Cargo Packager uses
that suffix to assign the correct Retina density in the ICNS container.

The artwork mirrors the interface wordmark: two foreground lowercase `t`
shapes followed by one accent-colored `t`, with the `DEV` badge keeping this
local build visibly separate from the deployed app. Keep that semantic
relationship and the badge if the geometry changes.

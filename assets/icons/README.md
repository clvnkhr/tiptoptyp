# Application icon

`tiptoptyp.svg` is the canonical production mark. `tiptoptyp-dev.svg` is the
development-only variant with an amber `DEV` badge. Each build has its own
deterministic raster set; Cargo Packager builds `tiptoptyp.icns` from the
selected PNG set when producing a macOS bundle, so an independently generated
ICNS file is intentionally not checked in. The 1024-pixel raster keeps the
`512@2x` suffix because Cargo Packager uses that suffix to assign the correct
Retina density in the ICNS container.

The artwork mirrors the interface wordmark: two foreground lowercase `t`
shapes followed by one accent-colored `t`. Keep that semantic relationship in
both variants if the geometry changes; keep the badge only on the development
variant.

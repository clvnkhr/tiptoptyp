#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
latest_directory="${repository_root}/docs/ui-snapshots/latest"
default_fixture="${repository_root}/docs/ui-snapshots/theme-fixture.typ"

usage() {
  cat <<'EOF'
Usage: scripts/capture-theme-gallery.sh [fixture]
       scripts/capture-theme-gallery.sh --print-manifest
       scripts/capture-theme-gallery.sh --validate-latest

Capture the maintained app-framebuffer theme gallery. The optional fixture
defaults to docs/ui-snapshots/theme-fixture.typ.

--print-manifest   Print the requested stable PNG filenames without launching.
--validate-latest  Decode and validate every requested PNG without launching.

TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS controls the base whole-session
watchdog and defaults to 45 seconds. The runner adds one second per image.
EOF
}

operation="capture"
case "${1:-}" in
  --print-manifest|--validate-latest)
    operation="${1#--}"
    shift
    ;;
  --help|-h)
    usage
    exit 0
    ;;
  --*)
    printf 'Unknown gallery option: %s\n' "$1" >&2
    usage >&2
    exit 2
    ;;
esac

if [[ "${operation}" == "capture" ]]; then
  if (( $# > 1 )); then
    usage >&2
    exit 2
  fi
  fixture="${1:-${default_fixture}}"
else
  if (( $# != 0 )); then
    usage >&2
    exit 2
  fi
  fixture="${default_fixture}"
fi

capture_timeout_seconds="${TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS:-45}"
case "${capture_timeout_seconds}" in
  ''|*[!0-9]*)
    printf 'Gallery capture timeout must be a whole number of seconds: %s\n' \
      "${capture_timeout_seconds}" >&2
    exit 2
    ;;
esac
if (( capture_timeout_seconds < 1 || capture_timeout_seconds > 3600 )); then
  printf 'Gallery capture timeout must be between 1 and 3600 seconds: %s\n' \
    "${capture_timeout_seconds}" >&2
  exit 2
fi

# Keep this list in the same canonical order as the built-in theme catalog.
all_builtin_themes=(
  tiptop-light
  tiptop-dark
  paper-light
  paper-dark
  ocean-light
  ocean-dark
  forest-light
  forest-dark
  catppuccin-latte
  catppuccin-frappe
  catppuccin-macchiato
  catppuccin-mocha
  solarized-light
  solarized-dark
  gruvbox-light
  gruvbox-dark
  github-light-default
  github-dark-default
  rose-pine-dawn
  rose-pine
  tokyo-night-light
  tokyo-night
  kanagawa-lotus
  kanagawa-wave
  everforest-light
  everforest-dark
  ayu-light
  ayu-dark
  flexoki-light
  flexoki-dark
  dracula-alucard
  dracula
)
themes=("${all_builtin_themes[@]}")

# These component families have stable checked-in PNG slots and run against one
# representative light theme and one representative dark theme by default.
# Other deterministic scenes remain available for targeted captures; adding a
# new scene to this list is intentionally paired with committing its PNGs.
scenes=(
  problems-panel
  find-replace
  preview-compiling
  file-menu
  edit-menu
  editor-context-menu
  explorer-context-menu
  settings-window
  settings-theme-picker
  settings-dark-theme-picker
  settings-tooltip
  diagnostic-tooltip
  save-dialog
  alert-dialog
  overwrite-dialog
  rename-dialog
  workspace-chooser
)

scene_themes=(
  catppuccin-latte
  catppuccin-mocha
)

# Subset overrides are useful for quick local review. They deliberately disable
# pruning: a partial run must never decide that the rest of the gallery is stale.
full_default_matrix=1
if [[ -n "${TIPTOPTYP_UI_GALLERY_THEMES:-}" ]]; then
  read -r -a themes <<< "${TIPTOPTYP_UI_GALLERY_THEMES}"
  full_default_matrix=0
fi
if [[ -n "${TIPTOPTYP_UI_GALLERY_SCENES:-}" ]]; then
  read -r -a scenes <<< "${TIPTOPTYP_UI_GALLERY_SCENES}"
  full_default_matrix=0
fi
if [[ -n "${TIPTOPTYP_UI_GALLERY_SCENE_THEMES:-}" ]]; then
  read -r -a scene_themes <<< "${TIPTOPTYP_UI_GALLERY_SCENE_THEMES}"
  full_default_matrix=0
fi
if [[ "${TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS:-0}" == "1" ]]; then
  full_default_matrix=0
fi

is_builtin_theme() {
  local candidate
  for candidate in "${all_builtin_themes[@]}"; do
    if [[ "$1" == "${candidate}" ]]; then
      return 0
    fi
  done
  return 1
}

scene_target() {
  case "$1" in
    main|problems-panel|find-replace|preview-compiling)
      printf 'main\n'
      ;;
    file-menu|edit-menu|editor-context-menu|explorer-context-menu|document-font-selector|status-log)
      printf 'popup\n'
      ;;
    settings-window|settings-theme-picker|settings-dark-theme-picker|settings-tooltip)
      printf 'settings\n'
      ;;
    typst-overrides-window)
      printf 'typst-overrides\n'
      ;;
    diagnostic-tooltip|function-tooltip)
      printf 'diagnostic\n'
      ;;
    save-dialog|alert-dialog|overwrite-dialog)
      printf 'modal\n'
      ;;
    rename-dialog)
      printf 'rename\n'
      ;;
    workspace-chooser)
      printf 'workspace\n'
      ;;
    *)
      return 1
      ;;
  esac
}

validate_requested_values() {
  local value
  if (( ${#themes[@]} == 0 || ${#scene_themes[@]} == 0 || ${#scenes[@]} == 0 )); then
    printf 'Gallery theme and scene lists must not be empty.\n' >&2
    return 1
  fi
  for value in "${themes[@]}" "${scene_themes[@]}"; do
    if ! is_builtin_theme "${value}"; then
      printf 'Unknown built-in gallery theme: %s\n' "${value}" >&2
      return 1
    fi
  done
  for value in "${scenes[@]}"; do
    if ! scene_target "${value}" >/dev/null; then
      printf 'Unknown gallery scene: %s\n' "${value}" >&2
      return 1
    fi
    if [[ "${value}" == "main" ]]; then
      printf 'The main scene belongs in the theme matrix, not the component scene list.\n' >&2
      return 1
    fi
  done
}

job_themes=()
job_scenes=()
job_inverts=()
job_hues=()
expected_outputs=()

expected_filename() {
  local theme="$1"
  local scene="$2"
  local invert="$3"
  local hue="$4"
  local target
  local view
  local theme_profile="${theme}"

  target="$(scene_target "${scene}")"
  if [[ "${target}" == "${scene}" ]]; then
    view="${target}"
  else
    view="${target}-${scene}"
  fi
  if [[ "${invert}" == "1" ]]; then
    theme_profile="${theme_profile}-inverted"
  fi
  if (( hue < 0 )); then
    theme_profile="${theme_profile}-hue-m$((-hue))"
  elif (( hue > 0 )); then
    theme_profile="${theme_profile}-hue-p${hue}"
  fi
  printf '%s--%s.png\n' "${view}" "${theme_profile}"
}

append_job() {
  local theme="$1"
  local scene="$2"
  local invert="$3"
  local hue="$4"
  local expected
  local existing

  expected="$(expected_filename "${theme}" "${scene}" "${invert}" "${hue}")"
  # The `+` form also works with `set -u` in macOS's Bash 3.2 while this
  # array is still empty for the first job.
  for existing in ${expected_outputs[@]+"${expected_outputs[@]}"}; do
    if [[ "${existing}" == "${expected}" ]]; then
      printf 'Duplicate gallery output requested: %s\n' "${expected}" >&2
      return 1
    fi
  done
  job_themes[${#job_themes[@]}]="${theme}"
  job_scenes[${#job_scenes[@]}]="${scene}"
  job_inverts[${#job_inverts[@]}]="${invert}"
  job_hues[${#job_hues[@]}]="${hue}"
  expected_outputs[${#expected_outputs[@]}]="${expected}"
}

build_manifest() {
  local theme
  local scene

  validate_requested_values
  for theme in "${themes[@]}"; do
    append_job "${theme}" main 0 0
  done
  if [[ "${TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS:-0}" != "1" ]]; then
    append_job catppuccin-latte main 1 30
  fi
  # Keep equal framebuffer targets adjacent. Immediate child viewports have
  # independent renderer state, so grouping avoids unnecessary root/child
  # churn while every image still comes from this one app session.
  for scene in "${scenes[@]}"; do
    for theme in "${scene_themes[@]}"; do
      append_job "${theme}" "${scene}" 0 0
    done
  done
  if [[ "${TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS:-0}" != "1" ]]; then
    append_job catppuccin-latte file-menu 1 30
  fi

  local expected_default_count
  expected_default_count=$((${#all_builtin_themes[@]} + ${#scene_themes[@]} * ${#scenes[@]} + 2))
  if (( full_default_matrix == 1 && ${#expected_outputs[@]} != expected_default_count )); then
    printf 'Internal gallery error: default manifest has %d outputs, expected %d.\n' \
      "${#expected_outputs[@]}" "${expected_default_count}" >&2
    return 1
  fi
}

png_decoder=""
select_png_decoder() {
  if command -v magick >/dev/null 2>&1; then
    png_decoder="magick"
  elif command -v convert >/dev/null 2>&1 && command -v identify >/dev/null 2>&1; then
    png_decoder="convert"
  elif command -v pngcheck >/dev/null 2>&1; then
    png_decoder="pngcheck"
  elif command -v sips >/dev/null 2>&1; then
    png_decoder="sips"
  else
    printf '%s\n' \
      'PNG validation needs ImageMagick, pngcheck, or macOS sips; none was found.' >&2
    return 1
  fi
}

validation_scratch=""
validate_png() {
  local path="$1"
  local signature

  if [[ ! -f "${path}" || ! -s "${path}" ]]; then
    printf 'Missing or empty gallery capture: %s\n' "${path}" >&2
    return 1
  fi
  signature="$(od -An -tx1 -N8 "${path}" | tr -d '[:space:]')"
  if [[ "${signature}" != "89504e470d0a1a0a" ]]; then
    printf 'Gallery capture is not a PNG: %s\n' "${path}" >&2
    return 1
  fi

  case "${png_decoder}" in
    magick)
      if ! magick "${path}" null: >/dev/null 2>&1; then
        printf 'Gallery PNG cannot be decoded: %s\n' "${path}" >&2
        return 1
      fi
      ;;
    convert)
      if ! convert "${path}" null: >/dev/null 2>&1; then
        printf 'Gallery PNG cannot be decoded: %s\n' "${path}" >&2
        return 1
      fi
      ;;
    pngcheck)
      if ! pngcheck -q "${path}" >/dev/null 2>&1; then
        printf 'Gallery PNG cannot be decoded: %s\n' "${path}" >&2
        return 1
      fi
      ;;
    sips)
      rm -f "${validation_scratch}"
      if ! sips -s format png "${path}" --out "${validation_scratch}" >/dev/null 2>&1 \
        || [[ ! -s "${validation_scratch}" ]]; then
        printf 'Gallery PNG cannot be decoded: %s\n' "${path}" >&2
        return 1
      fi
      ;;
    *)
      printf 'Internal gallery error: PNG decoder was not selected.\n' >&2
      return 1
      ;;
  esac
}

validate_manifest() {
  local expected
  for expected in "${expected_outputs[@]}"; do
    validate_png "${latest_directory}/${expected}"
  done
}

image_signature() {
  local path="$1"
  case "${png_decoder}" in
    magick)
      magick identify -quiet -format '%#' "${path}"
      ;;
    convert)
      identify -quiet -format '%#' "${path}"
      ;;
    *)
      return 1
      ;;
  esac
}

image_has_transparent_corners() {
  local path="$1"
  local value
  case "${png_decoder}" in
    magick)
      value="$(magick "${path}" -alpha extract \
        -format '%[fx:p{0,0}.r],%[fx:p{w-1,0}.r],%[fx:p{0,h-1}.r],%[fx:p{w-1,h-1}.r]' \
        info:)" || return 2
      ;;
    convert)
      value="$(convert "${path}" -alpha extract \
        -format '%[fx:p{0,0}.r],%[fx:p{w-1,0}.r],%[fx:p{0,h-1}.r],%[fx:p{w-1,h-1}.r]' \
        info:)" || return 2
      ;;
    *)
      return 2
      ;;
  esac
  [[ "${value}" == "0,0,0,0" ]]
}

is_expected_output() {
  local filename="$1"
  local expected
  for expected in "${expected_outputs[@]}"; do
    if [[ "${filename}" == "${expected}" ]]; then
      return 0
    fi
  done
  return 1
}

validate_visual_smoke() {
  local expected
  local path
  local corner_status
  local theme
  local settings_window
  local theme_picker
  local dark_theme_picker
  local settings_tooltip
  local settings_signature
  local picker_signature
  local dark_picker_signature
  local tooltip_signature

  if [[ "${png_decoder}" != "magick" && "${png_decoder}" != "convert" ]]; then
    printf 'Visual smoke checks skipped: %s validates PNG data but not pixels or alpha.\n' \
      "${png_decoder}" >&2
    return 0
  fi

  # Elevated child viewports must retain transparency around their themed
  # cards; opaque black corners are a regression even when the PNG decodes.
  for expected in "${expected_outputs[@]}"; do
    case "${expected}" in
      popup-*|diagnostic-*|modal-*|rename-*|workspace-*)
        path="${latest_directory}/${expected}"
        if image_has_transparent_corners "${path}"; then
          :
        else
          corner_status=$?
          case ${corner_status} in
            1)
              printf 'Elevated gallery capture has an opaque outer corner: %s\n' \
                "${path}" >&2
              ;;
            *)
              printf 'Could not inspect gallery capture corner transparency: %s\n' \
                "${path}" >&2
              ;;
          esac
          return 1
        fi
        ;;
    esac
  done

  # These Settings captures deliberately expose different themed state.
  # Comparing decoded pixel signatures catches a closed picker or missing card
  # without tying the snapshots to brittle, pre-recorded hashes.
  for theme in catppuccin-latte catppuccin-mocha; do
    settings_window="settings-settings-window--${theme}.png"
    theme_picker="settings-settings-theme-picker--${theme}.png"
    dark_theme_picker="settings-settings-dark-theme-picker--${theme}.png"
    settings_tooltip="settings-settings-tooltip--${theme}.png"
    if ! is_expected_output "${settings_window}" \
      || ! is_expected_output "${theme_picker}" \
      || ! is_expected_output "${dark_theme_picker}" \
      || ! is_expected_output "${settings_tooltip}"; then
      continue
    fi
    settings_signature="$(image_signature "${latest_directory}/${settings_window}")"
    picker_signature="$(image_signature "${latest_directory}/${theme_picker}")"
    dark_picker_signature="$(image_signature "${latest_directory}/${dark_theme_picker}")"
    tooltip_signature="$(image_signature "${latest_directory}/${settings_tooltip}")"
    if [[ "${settings_signature}" == "${picker_signature}" \
      || "${settings_signature}" == "${dark_picker_signature}" \
      || "${settings_signature}" == "${tooltip_signature}" \
      || "${picker_signature}" == "${dark_picker_signature}" \
      || "${picker_signature}" == "${tooltip_signature}" \
      || "${dark_picker_signature}" == "${tooltip_signature}" ]]; then
      printf 'Settings gallery states are pixel-identical for theme %s.\n' \
        "${theme}" >&2
      return 1
    fi
  done
}

prune_obsolete_pngs() {
  local candidate
  local filename
  local removed=0

  for candidate in "${latest_directory}"/*.png; do
    [[ -e "${candidate}" || -L "${candidate}" ]] || continue
    filename="${candidate##*/}"
    if ! is_expected_output "${filename}"; then
      rm -f -- "${candidate}"
      printf 'Removed obsolete gallery PNG: %s\n' "${filename}"
      removed=$((removed + 1))
    fi
  done
  printf 'Gallery pruning complete: %d obsolete PNG(s) removed.\n' "${removed}"
}

backup_directory=""
restore_outputs_on_exit=0
backup_completed=0
backed_up_outputs=()

restore_requested_outputs() {
  local expected
  local restore_failed=0
  local backed_up

  # Once backup completed, every requested destination belongs to this failed
  # transaction, including newly created slots with no original.
  if (( backup_completed == 1 )); then
    for expected in "${expected_outputs[@]}"; do
      if ! rm -f -- "${latest_directory}/${expected}"; then
        printf 'Could not clear failed gallery output before recovery: %s\n' \
          "${latest_directory}/${expected}" >&2
        restore_failed=1
      fi
    done
  fi

  for backed_up in ${backed_up_outputs[@]+"${backed_up_outputs[@]}"}; do
    if [[ -e "${backup_directory}/${backed_up}" \
      || -L "${backup_directory}/${backed_up}" ]]; then
      if (( backup_completed == 0 )) \
        && ! rm -f -- "${latest_directory}/${backed_up}"; then
        printf 'Could not clear partial gallery output before recovery: %s\n' \
          "${latest_directory}/${backed_up}" >&2
        restore_failed=1
        continue
      fi
      if ! mv -- "${backup_directory}/${backed_up}" \
      "${latest_directory}/${backed_up}"; then
        printf 'Could not restore gallery original: %s\n' \
          "${latest_directory}/${backed_up}" >&2
        restore_failed=1
      fi
    elif [[ ! -e "${latest_directory}/${backed_up}" \
      && ! -L "${latest_directory}/${backed_up}" ]]; then
      printf 'Gallery original and recovery backup are both missing: %s\n' \
        "${latest_directory}/${backed_up}" >&2
      restore_failed=1
    fi
  done
  return "${restore_failed}"
}

backup_requested_outputs() {
  local expected
  # Activate recovery before the first move. The completed-move ledger keeps
  # cleanup from deleting originals that have not yet reached the backup.
  restore_outputs_on_exit=1
  for expected in "${expected_outputs[@]}"; do
    if [[ -e "${latest_directory}/${expected}" \
      || -L "${latest_directory}/${expected}" ]]; then
      backed_up_outputs[${#backed_up_outputs[@]}]="${expected}"
      mv -- "${latest_directory}/${expected}" "${backup_directory}/${expected}"
    fi
  done
  backup_completed=1
}

cleanup() {
  local status=$?
  local recovery_failed=0
  trap - EXIT
  if (( restore_outputs_on_exit == 1 )); then
    if ! restore_requested_outputs; then
      recovery_failed=1
      status=1
    fi
  fi
  if (( recovery_failed == 0 )) \
    && [[ -n "${backup_directory}" && -d "${backup_directory}" ]]; then
    rm -rf -- "${backup_directory}"
  elif (( recovery_failed == 1 )); then
    printf 'Gallery recovery was incomplete; originals remain in %s\n' \
      "${backup_directory}" >&2
  fi
  exit "${status}"
}

app_binary=""

build_app() {
  local target_directory="${CARGO_TARGET_DIR:-${repository_root}/target}"
  if [[ "${target_directory}" != /* ]]; then
    target_directory="${repository_root}/${target_directory}"
  fi
  if ! command -v perl >/dev/null 2>&1; then
    printf 'The gallery watchdog requires Perl, but it was not found.\n' >&2
    return 1
  fi

  printf 'Building the release app once for %d gallery captures.\n' \
    "${#expected_outputs[@]}"
  cargo build --release --locked --bin tiptoptyp
  app_binary="${target_directory}/release/tiptoptyp"
  if [[ ! -x "${app_binary}" ]]; then
    printf 'Built app binary was not found or executable: %s\n' \
      "${app_binary}" >&2
    return 1
  fi
}

run_with_watchdog() {
  # macOS does not ship GNU `timeout`. `alarm` survives `exec`, so this keeps
  # the GUI app in the original process instead of forking it from Perl (which
  # makes AppKit window servicing intermittent). SIGALRM terminates a hung app.
  local timeout_seconds="$1"
  shift
  LC_ALL=C perl -e '
    use strict;
    use warnings;
    my $limit = shift @ARGV;
    alarm $limit;
    exec @ARGV;
    die "could not launch gallery app: $!\n";
  ' "${timeout_seconds}" "$@"
}

capture_gallery() {
  local index
  local invert
  local batch_timeout_seconds
  local command=(
    "${app_binary}"
    --ui-screenshot-latest
    --ui-screenshot-exit
    --ui-screenshot-settle 30
  )

  index=0
  while (( index < ${#expected_outputs[@]} )); do
    if [[ "${job_inverts[index]}" == "1" ]]; then
      invert=true
    else
      invert=false
    fi
    command+=(
      --ui-screenshot-step
      "${job_themes[index]},${job_scenes[index]},${invert},${job_hues[index]}"
    )
    index=$((index + 1))
  done
  command+=("${fixture}")

  batch_timeout_seconds=$((capture_timeout_seconds + ${#expected_outputs[@]}))
  printf 'Capturing %d gallery images in one app session (watchdog %d seconds).\n' \
    "${#expected_outputs[@]}" "${batch_timeout_seconds}"
  local capture_status=0
  if run_with_watchdog "${batch_timeout_seconds}" "${command[@]}"; then
    capture_status=0
  else
    capture_status=$?
  fi
  if (( capture_status != 0 )); then
    if (( capture_status == 142 )); then
      printf 'Gallery session timed out after %s seconds.\n' \
        "${batch_timeout_seconds}" >&2
    else
      printf 'Gallery session failed with status %d.\n' \
        "${capture_status}" >&2
    fi
    return 1
  fi
}

build_manifest

if [[ "${operation}" == "print-manifest" ]]; then
  printf '%s\n' "${expected_outputs[@]}"
  exit 0
fi

select_png_decoder

if [[ "${operation}" == "validate-latest" ]]; then
  mkdir -p "${repository_root}/.tiptoptyp"
  backup_directory="$(mktemp -d "${repository_root}/.tiptoptyp/theme-gallery-validation.XXXXXX")"
  validation_scratch="${backup_directory}/decoded.png"
  trap cleanup EXIT
  validate_manifest
  validate_visual_smoke
  printf 'Validated %d gallery PNGs with %s.\n' \
    "${#expected_outputs[@]}" "${png_decoder}"
  exit 0
fi

if [[ ! -f "${fixture}" ]]; then
  printf 'Theme gallery fixture does not exist: %s\n' "${fixture}" >&2
  exit 2
fi

mkdir -p "${latest_directory}" "${repository_root}/.tiptoptyp"
backup_directory="$(mktemp -d "${repository_root}/.tiptoptyp/theme-gallery.XXXXXX")"
validation_scratch="${backup_directory}/decoded.png"
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

cd "${repository_root}"
build_app
backup_requested_outputs
capture_gallery

# Revalidate the complete requested set after all app processes have exited.
validate_manifest
validate_visual_smoke

# Only a successful, unmodified default matrix is authoritative enough to
# remove old PNG slots. Dotfiles and non-PNG documentation are never touched.
if (( full_default_matrix == 1 )); then
  prune_obsolete_pngs
fi
restore_outputs_on_exit=0

printf 'Theme gallery complete: %d validated PNGs in %s\n' \
  "${#expected_outputs[@]}" "${latest_directory}"

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
By default, all scenes use Catppuccin Latte, with three additional Mocha
captures: the main window, File dropdown, and Save dialog popup.

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

manifest_path="${repository_root}/docs/ui-snapshots/gallery-manifest.tsv"
if [[ ! -f "${manifest_path}" ]]; then
  printf 'Gallery manifest does not exist: %s\n' "${manifest_path}" >&2
  exit 2
fi

catalog_theme_ids=()
catalog_theme_slugs=()
catalog_scene_ids=()
catalog_scene_targets=()
catalog_scene_stems=()
catalog_scene_roles=()
catalog_scene_corner_policies=()
catalog_scene_comparison_groups=()
scene_themes=()
variant_phases=()
variant_themes=()
variant_scenes=()
variant_inverts=()
variant_hues=()
variant_profile_slugs=()

manifest_token_is_safe() {
  case "$1" in
    ''|*[!a-zA-Z0-9._-]*)
      return 1
      ;;
    *)
      return 0
      ;;
  esac
}

contains_value() {
  local needle="$1"
  local existing
  shift
  for existing in "$@"; do
    if [[ "${existing}" == "${needle}" ]]; then
      return 0
    fi
  done
  return 1
}

manifest_line=0
while IFS=$'\t' read -r record first second third fourth fifth sixth extra \
  || [[ -n "${record:-}" ]]; do
  manifest_line=$((manifest_line + 1))
  [[ -z "${record}" || "${record}" == \#* ]] && continue
  case "${record}" in
    theme)
      if [[ -z "${first}" || -z "${second}" || -n "${third}" ]]; then
        printf 'Invalid theme record on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      if ! manifest_token_is_safe "${first}" \
        || ! manifest_token_is_safe "${second}"; then
        printf 'Unsafe theme token on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      if contains_value "${first}" \
        ${catalog_theme_ids[@]+"${catalog_theme_ids[@]}"}; then
        printf 'Duplicate theme id on gallery manifest line %d: %s\n' \
          "${manifest_line}" "${first}" >&2
        exit 2
      fi
      if contains_value "${second}" \
        ${catalog_theme_slugs[@]+"${catalog_theme_slugs[@]}"}; then
        printf 'Duplicate theme filename slug on gallery manifest line %d: %s\n' \
          "${manifest_line}" "${second}" >&2
        exit 2
      fi
      catalog_theme_ids[${#catalog_theme_ids[@]}]="${first}"
      catalog_theme_slugs[${#catalog_theme_slugs[@]}]="${second}"
      ;;
    scene)
      if [[ -z "${first}" || -z "${second}" || -z "${third}" \
        || -z "${fourth}" || -z "${fifth}" || -z "${sixth}" \
        || -n "${extra}" ]]; then
        printf 'Invalid scene record on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      if ! manifest_token_is_safe "${first}" \
        || ! manifest_token_is_safe "${second}" \
        || ! manifest_token_is_safe "${third}" \
        || ! manifest_token_is_safe "${fourth}" \
        || ! manifest_token_is_safe "${fifth}" \
        || ! manifest_token_is_safe "${sixth}"; then
        printf 'Unsafe scene token on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      if contains_value "${first}" \
        ${catalog_scene_ids[@]+"${catalog_scene_ids[@]}"}; then
        printf 'Duplicate scene id on gallery manifest line %d: %s\n' \
          "${manifest_line}" "${first}" >&2
        exit 2
      fi
      if contains_value "${third}" \
        ${catalog_scene_stems[@]+"${catalog_scene_stems[@]}"}; then
        printf 'Duplicate scene filename stem on gallery manifest line %d: %s\n' \
          "${manifest_line}" "${third}" >&2
        exit 2
      fi
      catalog_scene_ids[${#catalog_scene_ids[@]}]="${first}"
      catalog_scene_targets[${#catalog_scene_targets[@]}]="${second}"
      catalog_scene_stems[${#catalog_scene_stems[@]}]="${third}"
      catalog_scene_roles[${#catalog_scene_roles[@]}]="${fourth}"
      catalog_scene_corner_policies[${#catalog_scene_corner_policies[@]}]="${fifth}"
      catalog_scene_comparison_groups[${#catalog_scene_comparison_groups[@]}]="${sixth}"
      ;;
    scene-theme)
      if [[ -z "${first}" || -n "${second}" ]]; then
        printf 'Invalid scene-theme record on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      if ! manifest_token_is_safe "${first}"; then
        printf 'Unsafe scene-theme token on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      if contains_value "${first}" ${scene_themes[@]+"${scene_themes[@]}"}; then
        printf 'Duplicate scene theme on gallery manifest line %d: %s\n' \
          "${manifest_line}" "${first}" >&2
        exit 2
      fi
      scene_themes[${#scene_themes[@]}]="${first}"
      ;;
    variant)
      if [[ -z "${first}" || -z "${second}" || -z "${third}" \
        || -z "${fourth}" || -z "${fifth}" || -z "${sixth}" \
        || -n "${extra}" ]]; then
        printf 'Invalid variant record on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      if ! manifest_token_is_safe "${first}" \
        || ! manifest_token_is_safe "${second}" \
        || ! manifest_token_is_safe "${third}" \
        || ! manifest_token_is_safe "${sixth}"; then
        printf 'Unsafe variant token on gallery manifest line %d.\n' \
          "${manifest_line}" >&2
        exit 2
      fi
      case "${fourth}" in
        0|1)
          ;;
        *)
          printf 'Invalid variant inversion on gallery manifest line %d: %s\n' \
            "${manifest_line}" "${fourth}" >&2
          exit 2
          ;;
      esac
      numeric_hue="${fifth#-}"
      case "${numeric_hue}" in
        ''|*[!0-9]*)
          printf 'Invalid variant hue on gallery manifest line %d: %s\n' \
            "${manifest_line}" "${fifth}" >&2
          exit 2
          ;;
      esac
      if (( ${#numeric_hue} > 3 )) \
        || [[ "${numeric_hue}" != "0" && "${numeric_hue}" == 0* ]] \
        || [[ "${fifth}" == "-0" ]]; then
        printf 'Variant hue must use canonical decimal form on gallery manifest line %d: %s\n' \
          "${manifest_line}" "${fifth}" >&2
        exit 2
      fi
      # The canonical form above avoids Bash's legacy octal arithmetic rules.
      numeric_hue=$((10#${numeric_hue}))
      if [[ "${fifth}" == -* ]]; then
        normalized_hue=$((-numeric_hue))
      else
        normalized_hue=${numeric_hue}
      fi
      if (( normalized_hue < -180 || normalized_hue > 180 )); then
        printf 'Variant hue is outside -180..180 on gallery manifest line %d: %s\n' \
          "${manifest_line}" "${fifth}" >&2
        exit 2
      fi
      variant_phases[${#variant_phases[@]}]="${first}"
      variant_themes[${#variant_themes[@]}]="${second}"
      variant_scenes[${#variant_scenes[@]}]="${third}"
      variant_inverts[${#variant_inverts[@]}]="${fourth}"
      variant_hues[${#variant_hues[@]}]="${fifth}"
      variant_profile_slugs[${#variant_profile_slugs[@]}]="${sixth}"
      ;;
    *)
      printf 'Unknown gallery manifest record %s on line %d.\n' \
        "${record}" "${manifest_line}" >&2
      exit 2
      ;;
  esac
done < "${manifest_path}"

theme_slug=""
load_theme() {
  local candidate="$1"
  local index=0
  while (( index < ${#catalog_theme_ids[@]} )); do
    if [[ "${catalog_theme_ids[index]}" == "${candidate}" ]]; then
      theme_slug="${catalog_theme_slugs[index]}"
      return 0
    fi
    index=$((index + 1))
  done
  return 1
}

scene_target=""
scene_stem=""
scene_role=""
scene_corner_policy=""
scene_comparison_group=""
load_scene() {
  local candidate="$1"
  local index=0
  while (( index < ${#catalog_scene_ids[@]} )); do
    if [[ "${catalog_scene_ids[index]}" == "${candidate}" ]]; then
      scene_target="${catalog_scene_targets[index]}"
      scene_stem="${catalog_scene_stems[index]}"
      scene_role="${catalog_scene_roles[index]}"
      scene_corner_policy="${catalog_scene_corner_policies[index]}"
      scene_comparison_group="${catalog_scene_comparison_groups[index]}"
      return 0
    fi
    index=$((index + 1))
  done
  return 1
}

if (( ${#catalog_theme_ids[@]} == 0 || ${#catalog_scene_ids[@]} == 0 \
  || ${#scene_themes[@]} == 0 )); then
  printf 'Gallery manifest must declare themes, scenes, and scene themes.\n' >&2
  exit 2
fi

themes=(catppuccin-latte)
scenes=()
theme_matrix_scene=""
catalog_index=0
while (( catalog_index < ${#catalog_scene_ids[@]} )); do
  case "${catalog_scene_roles[catalog_index]}" in
    theme-matrix)
      if [[ -n "${theme_matrix_scene}" ]]; then
        printf 'Gallery manifest declares more than one theme-matrix scene.\n' >&2
        exit 2
      fi
      theme_matrix_scene="${catalog_scene_ids[catalog_index]}"
      ;;
    component)
      scenes[${#scenes[@]}]="${catalog_scene_ids[catalog_index]}"
      ;;
    targeted)
      ;;
    *)
      printf 'Unknown gallery role for scene %s: %s\n' \
        "${catalog_scene_ids[catalog_index]}" \
        "${catalog_scene_roles[catalog_index]}" >&2
      exit 2
      ;;
  esac
  case "${catalog_scene_corner_policies[catalog_index]}" in
    -|transparent)
      ;;
    *)
      printf 'Unknown corner policy for scene %s: %s\n' \
        "${catalog_scene_ids[catalog_index]}" \
        "${catalog_scene_corner_policies[catalog_index]}" >&2
      exit 2
      ;;
  esac
  catalog_index=$((catalog_index + 1))
done
if [[ -z "${theme_matrix_scene}" ]]; then
  printf 'Gallery manifest does not declare a theme-matrix scene.\n' >&2
  exit 2
fi

contract_outputs=()
append_contract_output() {
  local output="$1"
  if contains_value "${output}" ${contract_outputs[@]+"${contract_outputs[@]}"}; then
    printf 'Gallery manifest declares duplicate stable output: %s\n' \
      "${output}" >&2
    return 1
  fi
  contract_outputs[${#contract_outputs[@]}]="${output}"
}

validate_catalog_contract() {
  local index
  local theme
  local scene
  local magnitude
  local signed_hue
  local expected_profile

  load_scene "${theme_matrix_scene}"
  for theme in "${catalog_theme_ids[@]}"; do
    load_theme "${theme}"
    append_contract_output "${scene_stem}--${theme_slug}.png"
  done

  for theme in "${scene_themes[@]}"; do
    if ! load_theme "${theme}"; then
      printf 'Unknown built-in gallery scene theme: %s\n' "${theme}" >&2
      return 1
    fi
  done

  for scene in "${scenes[@]}"; do
    load_scene "${scene}"
    for theme in "${scene_themes[@]}"; do
      load_theme "${theme}"
      append_contract_output "${scene_stem}--${theme_slug}.png"
    done
  done

  index=0
  while (( index < ${#variant_phases[@]} )); do
    case "${variant_phases[index]}" in
      before-components|after-components)
        ;;
      *)
        printf 'Unknown gallery variant phase: %s\n' \
          "${variant_phases[index]}" >&2
        return 1
        ;;
    esac
    if ! load_theme "${variant_themes[index]}"; then
      printf 'Unknown built-in gallery theme in variant: %s\n' \
        "${variant_themes[index]}" >&2
      return 1
    fi
    expected_profile="${theme_slug}"
    if [[ "${variant_inverts[index]}" == "1" ]]; then
      expected_profile="${expected_profile}-inverted"
    fi
    magnitude="${variant_hues[index]#-}"
    magnitude=$((10#${magnitude}))
    if [[ "${variant_hues[index]}" == -* ]]; then
      signed_hue=$((-magnitude))
    else
      signed_hue=${magnitude}
    fi
    if (( signed_hue < 0 )); then
      expected_profile="${expected_profile}-hue-m$((-signed_hue))"
    elif (( signed_hue > 0 )); then
      expected_profile="${expected_profile}-hue-p${signed_hue}"
    fi
    if [[ "${variant_profile_slugs[index]}" != "${expected_profile}" ]]; then
      printf 'Gallery variant profile slug mismatch: expected %s, found %s\n' \
        "${expected_profile}" "${variant_profile_slugs[index]}" >&2
      return 1
    fi
    if ! load_scene "${variant_scenes[index]}"; then
      printf 'Unknown gallery scene in variant: %s\n' \
        "${variant_scenes[index]}" >&2
      return 1
    fi
    append_contract_output "${scene_stem}--${expected_profile}.png"
    index=$((index + 1))
  done
}

validate_catalog_contract

# Keep the catalog complete for explicit overrides, but make routine evidence
# light-only apart from three representative dark surfaces. Transformed color
# variants are opt-in with TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS=0.
scene_themes=(catppuccin-latte)
skip_variants="${TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS:-1}"

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
if [[ -n "${TIPTOPTYP_UI_GALLERY_SKIP_VARIANTS:-}" ]]; then
  full_default_matrix=0
fi

validate_requested_values() {
  local value
  if (( ${#themes[@]} == 0 || ${#scene_themes[@]} == 0 || ${#scenes[@]} == 0 )); then
    printf 'Gallery theme and scene lists must not be empty.\n' >&2
    return 1
  fi
  for value in "${themes[@]}" "${scene_themes[@]}"; do
    if ! load_theme "${value}"; then
      printf 'Unknown built-in gallery theme: %s\n' "${value}" >&2
      return 1
    fi
  done
  for value in "${scenes[@]}"; do
    if ! load_scene "${value}"; then
      printf 'Unknown gallery scene: %s\n' "${value}" >&2
      return 1
    fi
    if [[ "${scene_role}" == "theme-matrix" ]]; then
      printf 'The main scene belongs in the theme matrix, not the component scene list.\n' >&2
      return 1
    fi
  done
}

job_themes=()
job_scenes=()
job_inverts=()
job_hues=()
job_profile_slugs=()
job_corner_policies=()
job_comparison_groups=()
expected_outputs=()

append_job() {
  local theme="$1"
  local scene="$2"
  local invert="$3"
  local hue="$4"
  local profile_slug="$5"
  local expected
  local existing

  if ! load_scene "${scene}"; then
    printf 'Unknown gallery scene in job: %s\n' "${scene}" >&2
    return 1
  fi
  expected="${scene_stem}--${profile_slug}.png"
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
  job_profile_slugs[${#job_profile_slugs[@]}]="${profile_slug}"
  job_corner_policies[${#job_corner_policies[@]}]="${scene_corner_policy}"
  job_comparison_groups[${#job_comparison_groups[@]}]="${scene_comparison_group}"
  expected_outputs[${#expected_outputs[@]}]="${expected}"
}

append_variants() {
  local requested_phase="$1"
  local index=0
  while (( index < ${#variant_phases[@]} )); do
    case "${variant_phases[index]}" in
      before-components|after-components)
        ;;
      *)
        printf 'Unknown gallery variant phase: %s\n' \
          "${variant_phases[index]}" >&2
        return 1
        ;;
    esac
    if ! load_theme "${variant_themes[index]}"; then
      printf 'Unknown built-in gallery theme in variant: %s\n' \
        "${variant_themes[index]}" >&2
      return 1
    fi
    if ! load_scene "${variant_scenes[index]}"; then
      printf 'Unknown gallery scene in variant: %s\n' \
        "${variant_scenes[index]}" >&2
      return 1
    fi
    if [[ "${variant_phases[index]}" == "${requested_phase}" ]]; then
      append_job \
        "${variant_themes[index]}" \
        "${variant_scenes[index]}" \
        "${variant_inverts[index]}" \
        "${variant_hues[index]}" \
        "${variant_profile_slugs[index]}"
    fi
    index=$((index + 1))
  done
}

build_manifest() {
  local theme
  local scene

  validate_requested_values
  for theme in "${themes[@]}"; do
    load_theme "${theme}"
    append_job "${theme}" "${theme_matrix_scene}" 0 0 "${theme_slug}"
  done
  if (( full_default_matrix == 1 )); then
    load_theme catppuccin-mocha
    append_job catppuccin-mocha "${theme_matrix_scene}" 0 0 "${theme_slug}"
  fi
  if [[ "${skip_variants}" != "1" ]]; then
    append_variants before-components
  fi
  # Keep equal framebuffer targets adjacent. Immediate child viewports have
  # independent renderer state, so grouping avoids unnecessary root/child
  # churn while every image still comes from this one app session.
  for scene in "${scenes[@]}"; do
    for theme in "${scene_themes[@]}"; do
      load_theme "${theme}"
      append_job "${theme}" "${scene}" 0 0 "${theme_slug}"
    done
    if (( full_default_matrix == 1 )) \
      && [[ "${scene}" == "file-menu" || "${scene}" == "save-dialog" ]]; then
      load_theme catppuccin-mocha
      append_job catppuccin-mocha "${scene}" 0 0 "${theme_slug}"
    fi
  done
  if [[ "${skip_variants}" != "1" ]]; then
    append_variants after-components
  fi

  local expected_default_count
  expected_default_count=$((${#scenes[@]} + 4))
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
  local index
  local other
  local group
  local comparison_signatures=()

  if [[ "${png_decoder}" != "magick" && "${png_decoder}" != "convert" ]]; then
    printf 'Visual smoke checks skipped: %s validates PNG data but not pixels or alpha.\n' \
      "${png_decoder}" >&2
    return 0
  fi

  # Elevated child viewports must retain transparency around their themed
  # cards; opaque black corners are a regression even when the PNG decodes.
  index=0
  while (( index < ${#expected_outputs[@]} )); do
    expected="${expected_outputs[index]}"
    path="${latest_directory}/${expected}"
    if [[ "${job_corner_policies[index]}" == "transparent" ]]; then
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
    fi
    group="${job_comparison_groups[index]}"
    if [[ "${group}" == "-" ]]; then
      comparison_signatures[index]=""
    else
      comparison_signatures[index]="$(image_signature "${path}")"
    fi
    index=$((index + 1))
  done

  # Captures in the same comparison group deliberately expose different
  # themed state. Pairwise decoded-pixel signatures catch a missing popup or
  # card without tying the gallery to fixed image hashes.
  index=0
  while (( index < ${#expected_outputs[@]} )); do
    group="${job_comparison_groups[index]}"
    if [[ "${group}" != "-" ]]; then
      other=0
      while (( other < index )); do
        if [[ "${job_comparison_groups[other]}" == "${group}" \
          && "${job_profile_slugs[other]}" == "${job_profile_slugs[index]}" \
          && "${comparison_signatures[other]}" == "${comparison_signatures[index]}" ]]; then
          printf 'Gallery captures in comparison group %s are pixel-identical for theme %s.\n' \
            "${group}" "${job_profile_slugs[index]}" >&2
          return 1
        fi
        other=$((other + 1))
      done
    fi
    index=$((index + 1))
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

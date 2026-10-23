"""Regression journeys driven only through native UI input.

The inspection socket supplies assertions and actual hit rectangles, never actions.
See docs/desktop-ui-audit.md for the lower-level tests these journeys promote.
"""

import math
from copy import deepcopy
from contextlib import contextmanager

from desktop_ui_tex_journeys import TexJourneys
from desktop_ui_image_journeys import ImageJourneys
from desktop_ui_newest_journeys import NewestJourneys
from desktop_ui_partial_journeys import PartialPreviewJourneys
from desktop_ui_snippet_journeys import SnippetJourneys
from desktop_ui_ime_journeys import ImeJourneys
from desktop_ui_palette_journeys import PaletteJourneys
from desktop_ui_jump_journeys import PreviewJumpJourneys
from desktop_ui_math_grid_journeys import MathGridJourneys
from desktop_ui_effects_journeys import EffectsPacingJourneys
from desktop_ui_version_journeys import VersionJourneys
from desktop_ui_liquid_journeys import LiquidJourneys
from desktop_ui_ltex_journeys import LtexJourneys
from desktop_ui_settings_links_journeys import SettingsLinksJourneys
from desktop_ui_latexdiff_journeys import LatexdiffJourneys
from desktop_ui_texdoc_journeys import TexdocJourneys
from desktop_ui_citation_journeys import CitationJourneys
from desktop_ui_asset_layout_journeys import AssetLayoutJourneys
from desktop_ui_inactive_preview_journeys import InactivePreviewJourneys

PARTICLE_STYLES = ("Explosion", "Shotgun", "Flamethrower", "Railgun", "Plasma")
PARTICLE_ACTIONS = ("Typing", "Copy", "Cut", "Paste", "Comment", "Undo", "Redo",
                    "Delete", "Newline", "Indent")

PREVIEW_SOURCE = '#set page(width: 200pt, height: 200pt, margin: 20pt)\n= First\nBlack text on white paper.\n#pagebreak()\n= Second\nMiddle page.\n#pagebreak()\n= Third\n#v(95pt)\nFinal needle.\n'


def help_target_visible(state, name):
    target = state["targets"].get(name)
    scroll = state["scrolls"].get("settings.help")
    return (target is not None and scroll is not None and target["enabled"]
            and target["age_ms"] <= 500 and target["frame"] == scroll["frame"])


def fingerprint(text):
    value = 0xcbf29ce484222325
    for byte in text.encode("utf-8"):
        value = ((value ^ byte) * 0x100000001b3) & 0xffffffffffffffff
    return f"{value:016x}"


def editor_effect_preferences(document):
    """Require the current complete per-action schema, with no legacy fallback."""
    settings = document.get("editor_effects")
    if not isinstance(settings, dict) or type(settings.get("enabled")) is not bool \
            or settings.get("density") not in ("Low", "Medium", "High"):
        raise AssertionError("editor effect settings are unavailable or invalid")
    actions = settings.get("actions")
    if not isinstance(actions, dict) or set(actions) != {action.lower() for action in PARTICLE_ACTIONS}:
        raise AssertionError("editor effect settings must identify every keyboard action")
    for action, effects in actions.items():
        if not isinstance(effects, dict) or set(effects) != {"particles", "shake", "flash", "glow"} \
                or effects["particles"] is not None and effects["particles"] not in PARTICLE_STYLES \
                or any(type(effects.get(effect)) is not bool for effect in ("shake", "flash", "glow")):
            raise AssertionError("invalid editor effects for " + action)
    return settings


def typing_particle_state(document):
    """Reject missing/stale-looking animation evidence rather than infer a pass."""
    state = document.get("typing_particles")
    settings = editor_effect_preferences(document)
    if not isinstance(state, dict):
        raise AssertionError("particle settings/inspection is unavailable")
    for field in ("active_count", "burst_count", "emitted_count", "max_live", "pool_limit", "lifetime_ms"):
        if type(state.get(field)) is not int or state[field] < 0:
            raise AssertionError("invalid particle counter: " + field)
    if not 0 < state["pool_limit"] <= 256 or not 0 < state["lifetime_ms"] <= 1000 \
            or state["active_count"] > state["max_live"] \
            or state["max_live"] > state["pool_limit"] \
            or state["max_live"] > state["emitted_count"] \
            or state["burst_count"] > state["emitted_count"]:
        raise AssertionError("particle work/lifetime is not bounded or counters are inconsistent")
    live_styles = state.get("live_styles")
    if not isinstance(live_styles, dict) or any(
            style not in PARTICLE_STYLES or type(count) is not int or count <= 0
            for style, count in live_styles.items()) \
            or sum(live_styles.values()) != state["active_count"] \
            or (state.get("last_style") not in PARTICLE_STYLES if state["burst_count"]
                else state.get("last_style") is not None):
        raise AssertionError("particle styles do not identify their actual live bursts")
    action_bursts = state.get("action_bursts")
    if not isinstance(action_bursts, dict) or any(
            action not in PARTICLE_ACTIONS or type(count) is not int or count <= 0
            for action, count in action_bursts.items()) \
            or sum(action_bursts.values()) != state["burst_count"] \
            or (state.get("last_action") not in PARTICLE_ACTIONS if state["burst_count"]
                else state.get("last_action") is not None):
        raise AssertionError("particle actions do not identify their actual bursts")
    if state["burst_count"]:
        if action_bursts.get(state["last_action"], 0) == 0:
            raise AssertionError("last particle action has no recorded burst")
        origin, clip, age = state.get("origin"), state.get("clip"), state.get("last_emission_age_ms")
        if not isinstance(origin, list) or len(origin) != 2 \
                or not isinstance(clip, list) or len(clip) != 4 \
                or not all(type(value) in (int, float) and math.isfinite(value)
                           for value in origin + clip) \
                or type(age) not in (int, float) or not math.isfinite(age) or age < 0 \
                or not clip[0] < clip[2] or not clip[1] < clip[3] \
                or not clip[0] <= origin[0] <= clip[2] \
                or not clip[1] <= origin[1] <= clip[3]:
            raise AssertionError("particle emission was not located inside its editor clip")
    if state["active_count"] and (not settings["enabled"] or not state["burst_count"]
                                  or state["last_emission_age_ms"] >= state["lifetime_ms"]):
        raise AssertionError("inactive/expired particles were reported as active")
    return state


def typing_particle_gpu_state(document):
    """Require actual native GPU callback evidence, not just CPU particle state."""
    state = typing_particle_state(document)
    gpu = state.get("gpu")
    if not isinstance(gpu, dict) or gpu.get("backend") != "instanced-glow" \
            or type(gpu.get("ready")) is not bool:
        raise AssertionError("native instanced particle GPU evidence is unavailable")
    for field in ("draws", "uploads", "uploaded_instances", "last_draw_instances"):
        if type(gpu.get(field)) is not int or gpu[field] < 0:
            raise AssertionError("invalid particle GPU counter: " + field)
    if gpu.get("error") is not None:
        raise AssertionError("particle GPU callback failed: " + str(gpu["error"]))
    if gpu["uploaded_instances"] > state["pool_limit"] \
            or gpu["last_draw_instances"] > state["pool_limit"]:
        raise AssertionError("particle GPU instance count exceeds the bounded pool")
    return gpu


def particle_action_rendered(document, before, action, expected):
    """Require one cause-specific emission and an actual new native GPU draw."""
    state, prior = typing_particle_state(document), typing_particle_state(before)
    gpu, prior_gpu = typing_particle_gpu_state(document), typing_particle_gpu_state(before)
    counts = dict(prior["action_bursts"])
    counts[action] = counts.get(action, 0) + 1
    style = editor_effect_preferences(before)["actions"][action.lower()]["particles"]
    return (state["active_count"] > 0 and state["burst_count"] == prior["burst_count"] + 1
            and state["last_action"] == action and state["action_bursts"] == counts
            and state["last_style"] == style and state["live_styles"] == {style: state["active_count"]}
            and gpu["ready"] and gpu["draws"] > prior_gpu["draws"]
            and gpu["uploads"] > prior_gpu["uploads"]
            and gpu["uploaded_instances"] > 0 and gpu["last_draw_instances"] > 0
            and document["source_fingerprint"] == fingerprint(expected)
            and document["source_bytes"] == len(expected.encode("utf-8"))
            and document["editor_keyboard_focused"])


def editor_feedback_state(document):
    """Require bounded, cause-tagged evidence for actual nonparticle feedback."""
    particles = typing_particle_state(document)
    settings = editor_effect_preferences(document)
    state = particles.get("feedback")
    effects = {"Shake", "Flash", "Glow"}
    if not isinstance(state, dict) or type(state.get("burst_count")) is not int \
            or state["burst_count"] < 0 or type(state.get("lifetime_ms")) is not int \
            or not 0 < state["lifetime_ms"] <= 1000:
        raise AssertionError("bounded editor feedback evidence is unavailable")
    counts = state.get("action_bursts")
    if not isinstance(counts, dict) or any(
            action not in PARTICLE_ACTIONS or type(count) is not int or count <= 0
            for action, count in counts.items()) or sum(counts.values()) != state["burst_count"]:
        raise AssertionError("editor feedback must identify each action exactly once")
    last, active = state.get("last_effects"), state.get("active_effects")
    if any(not isinstance(value, list) or any(item not in effects for item in value)
           or len(set(value)) != len(value) for value in (last, active)) \
            or not set(active) <= set(last):
        raise AssertionError("editor feedback effects are missing or invalid")
    offset = state.get("shake_offset")
    if not isinstance(offset, list) or len(offset) != 2 or any(
            type(value) not in (int, float) or not math.isfinite(value) or abs(value) > 32
            for value in offset) or any(
            type(state.get(field)) not in (int, float) or not math.isfinite(state[field])
            or not 0 <= state[field] <= 1 for field in ("flash_alpha", "glow_alpha")):
        raise AssertionError("editor feedback render amplitudes are unbounded or invalid")
    age = state.get("last_emission_age_ms")
    if state["burst_count"]:
        if state.get("last_action") not in counts or not last \
                or type(age) not in (int, float) or not math.isfinite(age) or age < 0:
            raise AssertionError("editor feedback has no valid last action/emission")
    elif state.get("last_action") is not None or last or active or age is not None:
        raise AssertionError("empty editor feedback reports an emission")
    if active and (not settings["enabled"] or age >= state["lifetime_ms"]):
        raise AssertionError("disabled or expired editor feedback is active")
    if ("Shake" not in active and offset != [0, 0]) \
            or ("Flash" not in active and state["flash_alpha"] != 0) \
            or ("Glow" not in active and state["glow_alpha"] != 0):
        raise AssertionError("inactive editor feedback still changes its rendered appearance")
    return state


def editor_feedback_action_rendered(document, before, action, effect, expected):
    """A particle-Off action must render only its chosen finite feedback."""
    state, prior = editor_feedback_state(document), editor_feedback_state(before)
    particles, prior_particles = typing_particle_state(document), typing_particle_state(before)
    gpu, prior_gpu = typing_particle_gpu_state(document), typing_particle_gpu_state(before)
    settings = editor_effect_preferences(before)
    chosen = settings["actions"][action.lower()]
    expected_choice = {"particles": None, "shake": effect == "Shake", "flash": effect == "Flash",
                       "glow": effect == "Glow"}
    counts = dict(prior["action_bursts"])
    counts[action] = counts.get(action, 0) + 1
    amplitude = {"Shake": any(abs(value) > 0.01 for value in state["shake_offset"]),
                 "Flash": state["flash_alpha"] > 0, "Glow": state["glow_alpha"] > 0}[effect]
    return (settings["enabled"] and chosen == expected_choice
            and state["burst_count"] == prior["burst_count"] + 1
            and state["last_action"] == action and state["action_bursts"] == counts
            and state["last_effects"] == [effect] and state["active_effects"] == [effect] and amplitude
            and all(particles[field] == prior_particles[field] for field in (
                "burst_count", "emitted_count", "action_bursts"))
            and particles["active_count"] == 0
            and all(gpu[field] == prior_gpu[field] for field in ("draws", "uploads"))
            and document["source_fingerprint"] == fingerprint(expected)
            and document["source_bytes"] == len(expected.encode("utf-8"))
            and document["editor_keyboard_focused"])


def particle_document_returned_quiet(document, before, expected):
    """A document switch resets its trace; it must not replay a hidden command."""
    state = typing_particle_state(document)
    feedback = editor_feedback_state(document)
    gpu, prior_gpu = typing_particle_gpu_state(document), typing_particle_gpu_state(before)
    return (feedback["burst_count"] == 0 and not feedback["active_effects"]
            and document["active_tab_id"] == before["active_tab_id"]
            and document["source_fingerprint"] == fingerprint(expected)
            and document["source_bytes"] == len(expected.encode("utf-8"))
            and all(state[field] == 0 for field in (
                "active_count", "burst_count", "emitted_count", "max_live"))
            and state["last_style"] is None and state["last_action"] is None
            and state["live_styles"] == {} and state["action_bursts"] == {}
            and gpu["draws"] == prior_gpu["draws"])


def editor_particle_popup_target(state):
    """A selection has a current IME anchor, not a collapsed-caret target."""
    document, targets = state["document"], state["targets"]
    cursor = document["cursor"]
    name = "editor.ime_cursor" if cursor[0] != cursor[1] else "editor.caret"
    target, source = targets.get(name), targets.get("editor.source")
    if not document["editor_keyboard_focused"] or any(
            not value or not value.get("enabled") or value.get("age_ms", 501) > 500
            or value.get("frame") != document["frame"] for value in (target, source)) \
            or target.get("viewport") != source.get("viewport"):
        raise AssertionError("source popup needs a fresh focused cursor/selection anchor")
    if any(type(value.get(key)) not in (int, float) or not math.isfinite(value[key])
           for value, keys in ((source, ("x", "y", "width", "height")), (target, ("x", "y")))
           for key in keys) or source["width"] <= 0 or source["height"] <= 0 \
            or not source["x"] - source["width"] / 2 <= target["x"] <= source["x"] + source["width"] / 2 \
            or not source["y"] - source["height"] / 2 <= target["y"] <= source["y"] + source["height"] / 2:
        raise AssertionError("source popup cursor/selection anchor is outside its visible editor")
    return name


def settings_category_target_visible(state, category):
    target = state["targets"].get("settings.category." + category)
    search = state["targets"].get("settings.search")
    return bool(target and search and target["enabled"] and target["age_ms"] <= 500
                and target["frame"] == search["frame"] and target["viewport"] == search["viewport"])


def settings_json_editor_selection(state, draft):
    """Acknowledge keys only in the fresh focused editor for this exact draft."""
    settings = state.get("document", {}).get("settings_json", {})
    target = state.get("targets", {}).get("settings.json")
    if not isinstance(target, dict) or settings.get("visible") is not True \
            or settings.get("fingerprint") != draft:
        return None
    age, selection = target.get("age_ms"), target.get("selection")
    if target.get("enabled") is not True or target.get("focused") is not True \
            or type(age) not in (int, float) or not 0 <= age <= 500 \
            or not isinstance(selection, list) or len(selection) != 2 \
            or not all(type(position) is int and position >= 0 for position in selection):
        return None
    return selection


def settings_navigation_matches(document, category, query, source):
    return (document["settings_visible"] and not document["settings_json"]["visible"]
            and not document["settings_help_visible"] and document["settings_category"] == category
            and document["settings_query_fingerprint"] == fingerprint(query)
            and document["source_fingerprint"] == source)


def settings_window_size(windows):
    settings = [window for window in windows if "Settings" in window.get("AXTitle", "")]
    if len(settings) != 1:
        raise AssertionError("expected exactly one native Settings window")
    size = settings[0].get("size")
    if not isinstance(size, list) or len(size) != 2 or not all(
            isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value) and value > 0 for value in size):
        raise AssertionError("native Settings window size is missing or invalid")
    return tuple(size)


def native_size_matches(actual, expected):
    """macOS may round a restored content size by one logical point."""
    return (isinstance(actual, (list, tuple)) and isinstance(expected, (list, tuple))
            and len(actual) == len(expected) == 2
            and all(not isinstance(value, bool) and isinstance(value, (int, float))
                    and math.isfinite(value) and value > 0 for value in (*actual, *expected))
            and all(abs(a - b) <= 1 for a, b in zip(actual, expected)))


def pdfium_render_state(document):
    state = document.get("pdfium_render")
    if document.get("preview_backend") != "PDFium" or not isinstance(state, dict):
        raise AssertionError("missing effective PDFium render observation")
    for field in ("requested_scale", "desired_scale", "active_scale", "resident_count", "requests", "admissions"):
        value = state.get(field)
        minimum = 0 if field in ("requests", "admissions") else 1
        if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
            raise AssertionError("invalid PDFium render observation: " + field)
    if not isinstance(state.get("settling"), bool):
        raise AssertionError("missing PDFium settling state")
    return state


def assert_pdfium_resize_held(document, baseline):
    state = pdfium_render_state(document)
    if not state["settling"]:
        raise AssertionError("held PDFium divider did not defer sharp rendering")
    for field in ("requests", "admissions", "active_scale", "resident_count"):
        if state[field] != baseline[field]:
            raise AssertionError("held PDFium divider changed " + field)
    return state


def pdfium_resize_settled(document):
    state = document.get("pdfium_render") or {}
    return (document.get("preview_backend") == "PDFium"
            and document.get("preview_pdfium_ready") is True
            and state.get("settling") is False
            and isinstance(state.get("desired_scale"), int)
            and not isinstance(state["desired_scale"], bool) and state["desired_scale"] > 0
            and state.get("requested_scale") == state["desired_scale"]
            and state.get("active_scale") == state["desired_scale"]
            and isinstance(state.get("resident_count"), int)
            and not isinstance(state["resident_count"], bool) and state["resident_count"] > 0)


def background_fades_applied(document, blur_fade, transparency_fade,
                             keys=("native_background", "settings_native_background")):
    preferences = document.get("window_background") or {}
    if (preferences.get("blur_fade") != blur_fade
            or preferences.get("transparency_fade") != transparency_fade
            or preferences.get("enabled") is not True or preferences.get("blur") is not True):
        return False
    for field, maximum in (("blur_radius", 50), ("blur_end_radius", 50),
                           ("opacity_percent", 100), ("opacity_end_percent", 100)):
        value = preferences.get(field)
        if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= maximum:
            return False
    end_radius = preferences["blur_end_radius"] if blur_fade != "none" else 0
    end_opacity = preferences["opacity_end_percent"] if transparency_fade != "none" else 100
    if max(preferences["blur_radius"], end_radius) == 0 or min(preferences["opacity_percent"], end_opacity) == 100:
        return False
    identities = []
    for key in keys:
        background = document.get(key) or {}
        applied = background.get("blur") or {}
        if (background.get("translucent") is not True
                or background.get("blur_fade") != blur_fade
                or background.get("blur_radius") != preferences["blur_radius"]
                or applied.get("method") != ("window-server" if blur_fade == "none" else "variable-blur")
                or applied.get("applied_fade") != blur_fade
                or background.get("blur_end_radius") != end_radius
                or applied.get("applied_radius") != max(preferences["blur_radius"], end_radius)):
            return False
        for field in ("identity", "applications"):
            value = applied.get(field)
            if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                return False
        identities.append(applied["identity"])
    return len(set(identities)) == len(identities)


def background_blur_unchanged(document, previous,
                              keys=("native_background", "settings_native_background")):
    for key in keys:
        current = (document.get(key) or {}).get("blur") or {}
        before = (previous.get(key) or {}).get("blur") or {}
        for field in ("identity", "applications", "applied_radius", "applied_fade", "method"):
            if field not in before or current.get(field) != before[field]:
                return False
    return True


def assert_explorer_frame(state, independent, preview_edge):
    document = state["document"]
    frame = document["frame"]
    explorer = state["targets"].get("divider.explorer")
    if not document["explorer_visible"] and explorer and explorer["frame"] == frame:
        raise AssertionError("closed Explorer still painted an empty panel frame")
    editor = state["targets"].get("divider.editor")
    if independent:
        if not editor or editor["frame"] != frame:
            raise AssertionError("missing fresh independent preview edge")
        if not all(math.isfinite(edge) for edge in (editor["x"], preview_edge)):
            raise AssertionError("invalid independent preview edge geometry")
        if abs(editor["x"] - preview_edge) > 1:
            raise AssertionError("independent preview edge moved during Explorer toggle")
    return frame


def assert_git_overview(document, changed_lines=(5, 85, 165), line_count=181):
    """Validate the observed painted strip against the whole fixture, not its scroll position."""
    observation = document.get("git_overview")
    if not isinstance(observation, dict):
        raise AssertionError("missing freshly painted Git overview")

    def rectangle(value, name):
        if (not isinstance(value, (list, tuple)) or len(value) != 4
                or any(isinstance(edge, bool) or not isinstance(edge, (int, float))
                       or not math.isfinite(edge) for edge in value)
                or value[2] <= value[0] or value[3] <= value[1]):
            raise AssertionError("invalid Git overview " + name)
        return tuple(value)

    def contains(outer, inner):
        return (inner[0] >= outer[0] - 0.5 and inner[1] >= outer[1] - 0.5
                and inner[2] <= outer[2] + 0.5 and inner[3] <= outer[3] + 0.5)

    viewport = rectangle(observation.get("viewport"), "viewport")
    track = rectangle(observation.get("track"), "track")
    pane = rectangle(document.get("editor_panel_rect"), "code pane")
    width = document.get("editor_panel_width")
    if (isinstance(width, bool) or not isinstance(width, (int, float))
            or not math.isfinite(width) or width <= 0 or viewport[2] - viewport[0] > width + 1
            or not contains(pane, viewport)):
        raise AssertionError("Git overview extends beyond its code pane")
    if (not contains(viewport, track) or track[2] - track[0] > 4.01
            or abs(track[1] - viewport[1]) > 0.5 or abs(track[3] - viewport[3]) > 0.5
            or not 0 <= viewport[2] - track[2] <= 2):
        raise AssertionError("Git overview is not a thin strip inside the code pane's right edge")
    ticks = observation.get("ticks")
    if not isinstance(ticks, list) or len(ticks) != len(changed_lines):
        raise AssertionError("Git overview must include every change, including offscreen hunks")
    marks = []
    for tick in ticks:
        if not isinstance(tick, dict) or tick.get("kind") != "Modified":
            raise AssertionError("Git overview has the wrong change kind for this fixture")
        rect = rectangle(tick.get("rect"), "tick")
        if not contains(track, rect):
            raise AssertionError("Git overview tick escapes its strip")
        marks.append(rect)
    marks.sort(key=lambda rect: rect[1])
    height = track[3] - track[1]
    # Allow one logical line for the final empty row and pixel rounding. A
    # viewport-relative or clipped list of gutter marks cannot satisfy this.
    tolerance = max(2, height / line_count)
    for rect, line in zip(marks, changed_lines):
        if abs(rect[1] - (track[1] + height * line / line_count)) > tolerance:
            raise AssertionError("Git overview tick does not represent its global document position")
    return {"viewport": viewport, "track": track, "ticks": tuple(marks)}


def finish_clipboard_adapter(process):
    """Bound restoration and stop/reap a timed-out adapter."""
    import subprocess
    try:
        _, error = process.communicate("restore\n", timeout=6)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            process.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            try:
                process.communicate(timeout=2)
            except subprocess.TimeoutExpired:
                raise RuntimeError("clipboard adapter could not be reaped; restoration could not be verified") from None
        raise RuntimeError("clipboard adapter timed out; restoration could not be verified") from None
    if process.returncode:
        raise RuntimeError(error or "clipboard adapter failed")


class EditorJourneys(SettingsLinksJourneys, LtexJourneys, InactivePreviewJourneys, AssetLayoutJourneys, CitationJourneys, TexdocJourneys, LatexdiffJourneys, LiquidJourneys, VersionJourneys, TexJourneys, ImageJourneys, NewestJourneys, PartialPreviewJourneys, SnippetJourneys, ImeJourneys, PaletteJourneys, PreviewJumpJourneys, MathGridJourneys, EffectsPacingJourneys):
    def set_math_conversion_formatting(self, enabled):
        if self.snapshot()["document"]["format_after_math_conversion"] is enabled:
            return
        self.settings()
        self.setting_section("format after math conversion", "Format after math conversion")
        self.click("settings.editor.math_format")
        self.wait("math conversion formatting " + ("enabled" if enabled else "disabled"),
                  lambda d: d["format_after_math_conversion"] is enabled)
        self.close_settings()

    @contextmanager
    def exact_math_conversion(self):
        # Delimiter and undo checks exercise conversion independently of the
        # optional formatter. Change the real setting and restore its value.
        original = self.snapshot()["document"]["format_after_math_conversion"]
        if not isinstance(original, bool):
            raise AssertionError("missing effective math conversion formatting setting")
        self.set_math_conversion_formatting(False)
        try:
            yield
        finally:
            self.set_math_conversion_formatting(original)

    def focus_document_window(self):
        if self.snapshot()["document"]["focused"]:
            return
        windows = self.native("windows").get("windows", [])
        title = next((window.get("AXTitle") for window in windows
                      if window.get("AXMain") and not window.get("AXMinimized")), None)
        self.native("activate")
        if title:
            self.native("raise", title)
        self.wait("document window focused", lambda d: d["focused"])

    def paste_source(self, text, check=None):
        import json
        import subprocess
        process = subprocess.Popen([str(self.driver), "paste", str(self.process.pid), text],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            import select
            if not select.select([process.stdout], [], [], 6)[0]:
                raise RuntimeError("paste adapter did not acknowledge input")
            receipt = process.stdout.readline()
            if not receipt or json.loads(receipt).get("posted") != "paste":
                raise RuntimeError("paste adapter failed before posting input")
            self.record("paste", bytes=len(text.encode("utf-8")), fingerprint=fingerprint(text))
            if check is None:
                self.source_is(text)
            else:
                check()
        finally:
            finish_clipboard_adapter(process)

    def copy_from_menu(self, expected):
        self.copy_with_receipt(expected, lambda: self.click("menu.Copy"))

    def copy_with_receipt(self, expected, perform, after_copy=None):
        """Check a real clipboard write and restore the human's prior clipboard."""
        import json
        import select
        import subprocess
        import time
        process = subprocess.Popen(
            [str(self.driver), "watch-copy", str(self.process.pid), expected],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )

        def receipt():
            if not select.select([process.stdout], [], [], 6)[0]:
                raise RuntimeError("clipboard adapter did not acknowledge Copy")
            line = process.stdout.readline()
            if not line:
                raise RuntimeError("clipboard adapter exited before acknowledging Copy")
            return json.loads(line)

        ready = False
        try:
            if receipt().get("ready") != "copy":
                raise RuntimeError("clipboard adapter did not start")
            ready = True
            perform()
            if after_copy is not None:
                after_copy()
            deadline = time.monotonic() + 6
            while True:
                process.stdin.write("check\n")
                process.stdin.flush()
                if receipt().get("matches"):
                    self.record("clipboard.copy", bytes=len(expected.encode("utf-8")),
                                fingerprint=fingerprint(expected))
                    return
                if time.monotonic() >= deadline:
                    raise AssertionError("Copy did not match the selected fixture text")
                time.sleep(0.05)
        finally:
            try:
                if ready and process.poll() is None:
                    # Arm restoration even if observing the effect failed
                    # before the ordinary clipboard-confirmation loop ran.
                    process.stdin.write("check\n")
                    process.stdin.flush()
                    receipt()
            finally:
                finish_clipboard_adapter(process)

    def review_native_preview(self, label, document_focus=True):
        """Optional observation boundary; a resumed pause is not a visual pass."""
        if not self.review_preview:
            return
        import time
        resume = self.evidence / (label + ".reviewed")
        print(f"Visual review: {label}; app={self.directory / 'Journey.app'}; resume={resume}", flush=True)
        deadline = time.monotonic() + 180
        while not resume.exists():
            if self.process.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError("visual review not completed within three minutes")
            time.sleep(0.2)
        self.native("activate")
        if document_focus:
            self.wait("document focused after visual review", lambda d: d["focused"])

    def capture_initial_review(self):
        # Native activation alone may leave the first responder unclaimed.
        # Establish the initial source owner before the optional kickoff image.
        self.focus_document_window()
        self.wait_target("editor.source")
        self.click("editor.source")
        self.wait("source owns keyboard before initial review capture", lambda d:
                  d["focused"] and d["editor_keyboard_focused"])
        self.capture_viewport("main")

    def capture_viewport(self, target):
        import shutil
        import time
        directory = self.directory / ".tiptoptyp/screenshots"
        before = set(directory.rglob("*.png"))
        self.key(111, "cmd+shift")
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            fresh = set(directory.rglob("*.png")) - before
            matches = [path for path in fresh if target in path.name and path.read_bytes().endswith(b"\x00\x00\x00\x00IEND\xaeB`\x82")]
            if matches:
                for path in matches:
                    shutil.copy2(path, self.evidence / path.name)
                self.record("framebuffer.captured", target=target, files=[p.name for p in matches])
                return
            time.sleep(0.05)
        raise AssertionError("no fresh framebuffer for " + target)

    def source_is(self, text):
        return self.wait("exact fixture content", lambda d: d["source_fingerprint"] == fingerprint(text) and d["source_bytes"] == len(text.encode("utf-8")))

    def ensure_typesetting_tab(self):
        """Keep document-scoped journeys independent of dialog tab residue."""
        if (self.snapshot()["document"]["path"] or "").endswith((".typ", ".tex")):
            return
        for _ in range(4):
            self.key(48, "ctrl+shift")
            if (self.snapshot()["document"]["path"] or "").endswith((".typ", ".tex")):
                break
        self.wait("journey selects a typesetting document", lambda d: (d["path"] or "").endswith((".typ", ".tex")))

    def scratch(self, text=""):
        self.focus_document_window()
        count = self.snapshot()["document"]["tabs"]
        self.key(45)
        self.wait("new empty scratch tab", lambda d: d["tabs"] == count + 1 and d["path"] is None and d["source_bytes"] == 0)
        self.wait_target("editor.source")
        self.click("editor.source")
        if text:
            if "\n" in text:
                self.paste_source(text)
            else:
                self.native("text", text)
                self.source_is(text)
        return count

    def close_scratch(self, count):
        self.key(13)
        self.wait("scratch close either completes or asks for confirmation",
                  lambda d: d["tabs"] == count or d["modal_open"])
        if self.snapshot()["document"]["modal_open"]:
            self.wait_target("modal.discard")
            self.click("modal.discard")
        self.wait("scratch tab closed once", lambda d: d["tabs"] == count and not d["modal_open"])

    def editing(self):
        count = self.scratch()
        self.click("editor.source")
        self.native("text", "zzz")
        # Observe the edit and caret in one frame. Waiting for the source
        # first can allow an asynchronous selection update to move the caret
        # before the regression assertion samples it.
        self.wait("typed caret follows text", lambda d: d["source_fingerprint"] == fingerprint("zzz")
                  and d["cursor"] == [3, 3] and d["dirty"])
        self.stable_state(lambda d: d["source_fingerprint"] == fingerprint("zzz") and d["cursor"] == [3, 3])
        self.click("editor.source")
        self.key(6)  # Undo
        self.source_is("")
        self.wait("undo to saved contents clears unsaved state", lambda d: not d["dirty"])
        self.click("editor.source")
        self.key(123, "cmd")  # Move caret before replaying the edit.
        self.click("editor.source")
        self.key(6, "cmd+shift")
        self.source_is("zzz")
        self.wait("redo restores unsaved state", lambda d: d["dirty"])
        self.wait("redo restores post-edit caret", lambda d: d["cursor"] == [3, 3])
        self.native("text", "!")
        self.source_is("zzz!")
        self.key(0)
        self.native("text", "é🙂z")
        self.source_is("é🙂z")
        self.wait("Unicode scalar caret", lambda d: d["cursor"] == [3, 3])
        self.key(6)
        self.source_is("zzz!")
        self.key(6, "cmd+shift")
        self.source_is("é🙂z")
        self.wait("Unicode redo caret", lambda d: d["cursor"] == [3, 3])
        self.key(0)
        self.paste_source("  alpha\n\tbeta")
        self.source_is("  alpha\n\tbeta")
        self.key(0)
        self.key(44)  # Toggle comment without moving the indentation.
        self.source_is("  // alpha\n\t// beta")
        self.key(44)
        self.source_is("  alpha\n\tbeta")
        self.key(0)
        self.paste_source("one\ntwo\nthree")
        self.key(126, "cmd")  # Start of document.
        self.key(125, "shift")  # Caret at column zero of line two.
        self.wait("downward selection ends on next line", lambda d: d["cursor"] == [4, 0])
        self.key(44)
        self.source_is("// one\n// two\nthree")
        self.wait("comment preserves downward selection", lambda d: d["cursor"] == [10, 3])
        self.key(44)
        self.source_is("one\ntwo\nthree")
        self.wait("uncomment preserves downward selection", lambda d: d["cursor"] == [4, 0])
        self.key(124, "none")  # Collapse at line two's start.
        self.key(126, "shift")
        self.wait("upward selection includes preceding newline", lambda d: d["cursor"] == [0, 4])
        self.key(44)
        self.source_is("// one\ntwo\nthree")
        self.wait("comment preserves upward selection", lambda d: d["cursor"] == [3, 7])
        self.key(44)
        self.source_is("one\ntwo\nthree")
        self.wait("uncomment preserves upward selection", lambda d: d["cursor"] == [0, 4])
        self.wait_target("editor.line_start.1")
        self.drag("editor.line_start.1", "editor.line_start.0")
        self.wait("upward mouse selection includes preceding newline", lambda d: d["cursor"] == [0, 4])
        self.key(51, "none")
        self.source_is("two\nthree")  # Deleting the selection removes its newline too.
        self.key(6)
        self.source_is("one\ntwo\nthree")
        self.key(0)
        self.paste_source("one\n")
        self.key(126, "cmd")
        self.key(125, "shift")
        self.wait("blank endpoint selected", lambda d: d["cursor"] == [4, 0])
        self.key(44)
        self.source_is("// one\n// ")
        self.key(44)
        self.source_is("one\n")
        self.close_scratch(count)

    def search(self):
        text = "alpha beta alpha ALPHA"
        count = self.scratch(text)
        self.key(3)
        self.wait("Find focused", lambda d: d["find_focused"])
        self.native("text", "alpha")
        self.wait("query received", lambda d: d["find_query_fingerprint"] == fingerprint("alpha") and d["find_selected"] is not None)
        self.source_is(text)
        self.key(0)  # Select All must affect the query, never the source.
        self.native("text", "beta")
        self.wait("query replacement isolated", lambda d: d["find_query_fingerprint"] == fingerprint("beta"))
        self.source_is(text)
        self.key(0)
        self.native("text", "alpha")
        self.wait("search reset", lambda d: d["find_selected"] is not None)
        first = self.snapshot()["document"]["find_selected"]
        self.key(36, "none")
        self.wait("Enter advances match", lambda d: d["find_selected"] is not None and d["find_selected"] != first)
        self.key(36, "shift")
        self.wait("Shift+Enter returns match", lambda d: d["find_selected"] == first)
        self.key(8, "cmd+alt")
        self.wait("case toggle", lambda d: not d["find_case"])
        self.key(8, "cmd+alt")
        self.wait("case restores", lambda d: d["find_case"])
        self.key(7, "cmd+alt")
        self.wait("regex toggle", lambda d: d["find_regex"])
        self.key(7, "cmd+alt")
        self.wait("regex restores", lambda d: not d["find_regex"])
        self.click("find.replace")
        self.wait("replace fields open", lambda d: d["replace_visible"])
        self.wait_target("find.replacement")
        self.click("find.replacement")
        self.native("text", "omega")
        self.click("find.replace_one")
        self.wait("replace one edits only one occurrence", lambda d: d["source_fingerprint"] in
                  (fingerprint("omega beta alpha ALPHA"), fingerprint("alpha beta omega ALPHA")))
        self.click("find.replace_all")
        self.source_is("omega beta omega ALPHA")
        self.key(53, "none")
        self.wait("Escape closes Find", lambda d: not d["find_visible"])
        self.key(6)
        self.wait("replace all undoes once", lambda d: d["source_fingerprint"] in
                  (fingerprint("omega beta alpha ALPHA"), fingerprint("alpha beta omega ALPHA")))
        self.key(6)
        self.source_is(text)
        self.close_scratch(count)

    def tabs(self):
        original = self.snapshot()["document"]
        count = self.scratch("first scratch")
        self.key(123, "none")
        self.wait("caret moved", lambda d: d["cursor"] == [12, 12])
        self.scratch("second scratch")
        self.key(48, "ctrl+shift")
        self.source_is("first scratch")
        self.wait("previous tab restores caret", lambda d: d["cursor"] == [12, 12])
        self.key(48, "ctrl")
        self.source_is("second scratch")
        self.wait("next tab restores caret", lambda d: d["cursor"] == [14, 14])
        self.close_scratch(count + 1)
        self.source_is("first scratch")
        self.close_scratch(count)
        self.wait("original tab and pinned preview retained", lambda d: d["path"] == original["path"]
                  and d["source_fingerprint"] == original["source_fingerprint"] and d["preview_path"] == original["preview_path"])

    def tab_drag(self):
        count = self.scratch("first scratch")
        self.scratch("second scratch")
        before = self.snapshot()["document"]["tab_order"]
        if len(before) != count + 2:
            raise AssertionError(f"unexpected tab order before drag: {before}")
        source = f"tab.{before[-1]}"
        destination = f"tab.{before[0]}"
        self.drag(source, destination)
        self.wait("tab drag reorders the native strip", lambda d: d["tab_order"] == [before[-1], *before[:-1]])
        self.click(f"tab.{before[1]}")
        self.wait("first scratch remains selectable after drag", lambda d: d["active_tab_id"] == before[1])
        self.close_scratch(count + 1)
        self.click(f"tab.{before[-1]}")
        self.close_scratch(count)
        self.wait("tab drag preserves the original tab", lambda d: d["tabs"] == count and d["path"] is not None)

    def dialogs(self):
        opened = self.directory / "opened.tex"
        opened.write_text("\\documentclass{article}\n\\begin{document}\nOpened.\n\\end{document}\n")
        self.key(31, "cmd")  # Cmd+O
        import time
        time.sleep(0.8)  # rfd creates the native panel asynchronously.
        self.native("ax-press-description", "search")
        self.native("text", opened.name)
        time.sleep(0.8)
        self.native("ax-press-title", "Open")
        self.wait("native Open panel loads a TeX file", lambda d: d["path"] == str(opened), timeout=15)

        self.key(45)  # New empty document.
        self.wait("dialog journey gets an empty tab", lambda d: d["path"] is None and d["source_bytes"] == 0)
        destination = self.directory / "saved.arbitrary"
        self.save_to_fixture(destination)
        self.wait("Save As accepts an arbitrary extension", lambda d: d["path"] == str(destination), timeout=15)

    def startup_preview(self):
        source = self.directory / "journey.typ"
        self.wait("plain launch starts with an unbound startup buffer", lambda d: d["tabs"] == 1 and d["path"] is None and d["source_bytes"] > 0)
        self.key(31, "cmd")  # Cmd+O
        import time
        time.sleep(0.8)
        self.native("ax-press-description", "search")
        self.native("text", source.name)
        time.sleep(0.8)
        self.native("ax-press-title", "Open")
        self.wait("first opened source becomes active", lambda d: d["path"] == str(source), timeout=15)
        self.wait("first opened source becomes the preview target", lambda d: d["preview_path"] == str(source) and d["preview_tab_id"] == d["active_tab_id"], timeout=30)

    def diagnostics(self):
        current_path = self.snapshot()["document"]["path"] or ""
        if not current_path.endswith((".typ", ".tex")):
            # The dialog journey deliberately leaves an arbitrary-extension
            # tab active. Move back to the original Typst owner before
            # asking the compiler for diagnostics.
            for _ in range(2):
                self.key(48, "ctrl+shift")
            self.wait("diagnostic journey selects a typesetting tab", lambda d: (d["path"] or "").endswith(".typ"))
        self.key(35, "cmd+alt")
        self.wait("diagnostic source owns the preview", lambda d: d["preview_tab_id"] == d["active_tab_id"])
        self.choose_backend("Pdfium", save_pdf=False)
        before = self.snapshot()["document"]
        self.click("editor.source")
        self.key(0)
        self.paste_source("#let broken = {\n")
        self.key(23)  # Problems
        self.wait("Problems exposes compiler diagnostics", lambda d: d["panel"] == "Problems" and d["compiler_diagnostic_count"] > 0, timeout=30)
        self.wait_target("problem.0")
        self.double_click("problem.0")
        self.wait("Problems navigates to the original source, never a generated mirror",
                  lambda d: d["focused"] and d["panel"] == "Problems"
                  and d["path"] == before["path"] and d["active_tab_id"] == before["active_tab_id"]
                  and d["tabs"] == before["tabs"])
        self.stable_state(lambda d: d["path"] == before["path"] and d["active_tab_id"] == before["active_tab_id"] and d["tabs"] == before["tabs"])
        self.key(6, "cmd")
        self.wait("diagnostic fixture undo restores the prior source", lambda d: d["source_fingerprint"] == before["source_fingerprint"] and d["source_bytes"] == before["source_bytes"])
        self.key(23)
        self.wait("diagnostic journey closes its panel", lambda d: d["panel"] is None)

    def rectangle(self):
        # Optional local integration: preserve the user's bindings and settings.
        import pathlib
        import plistlib
        import subprocess
        subprocess.run(["pgrep", "-x", "Rectangle"], check=True, capture_output=True)
        preferences = plistlib.loads((pathlib.Path.home() / "Library/Preferences/com.knollsoft.Rectangle.plist").read_bytes())
        self.key(3)
        self.wait("Find focused before Rectangle shortcuts", lambda d: d["find_visible"] and d["find_focused"])
        for name in ("almostMaximize", "bottomLeftSixth"):
            binding = preferences[name]
            flags = "+".join(name for bit, name in ((1 << 20, "cmd"), (1 << 19, "alt"), (1 << 18, "ctrl"), (1 << 17, "shift")) if binding["modifierFlags"] & bit) or "none"
            before = self.snapshot()["document"]["native_size"]
            self.record("window-manager.shortcut", window_action=name, keycode=binding["keyCode"], modifiers=flags)
            self.key(binding["keyCode"], flags)
            self.wait("Rectangle " + name + " resizes Find's owner", lambda d: d["native_size"] != before and d["find_visible"] and d["find_focused"])
            self.stable_state(lambda d: d["find_visible"] and d["find_focused"])
        self.click("find.close")
        self.wait("Find still closes normally after Rectangle commands", lambda d: not d["find_visible"])

    def drawing(self):
        count = self.scratch()
        if not self.snapshot()["document"]["explorer_visible"]:
            self.key(18)
        self.click("explorer.maximize.Draw symbol")
        self.wait("drawing panel maximized", lambda d: d["explorer_maximized"] == "Draw symbol")
        self.wait_target("drawing.canvas")
        target = self.snapshot()["targets"]["drawing.canvas"]
        # Three genuine pen strokes form an A; no fixture state injection.
        x,y = target["x"],target["y"]
        for a,b in [((-55,70),(0,-70)), ((0,-70),(55,70)), ((-35,25),(35,25))]:
            self.native("drag", x+a[0], y+a[1], x+b[0], y+b[1])
        state = self.wait("drawing recognized", lambda d: len(d["drawing"]["strokes"]) == 3 and not d["drawing"]["busy"] and any(p["detexify"] for p in d["drawing"]["predictions"]), timeout=30)
        strokes = state["document"]["drawing"]["strokes"]
        self.click("drawing.undo")
        self.wait("Undo removes one stroke", lambda d: len(d["drawing"]["strokes"]) == 2)
        self.click("drawing.redo")
        self.wait("Redo restores exact ink", lambda d: d["drawing"]["strokes"] == strokes)
        self.native("resize-focused", 1100, 700)
        old_size = state["document"]["native_size"]
        self.wait("drawing survives resize", lambda d: d["native_size"] != old_size and d["drawing"]["strokes"] == strokes)
        self.wait_target("drawing.canvas")
        prediction = next(p for p in self.snapshot()["document"]["drawing"]["predictions"] if p["typst"] is not None)
        self.click("drawing.prediction." + str(prediction["index"]))
        self.source_is(prediction["typst"])
        self.key(6)
        self.source_is("")
        self.settings()
        self.setting_section("handwriting", "Handwriting recognition")
        self.click("settings.handwriting.Detypify")
        self.click("settings.handwriting.names")
        self.close_settings()
        state = self.wait("only Detypify re-recognizes retained ink", lambda d: d["drawing"]["engine"] == "detypify" and not d["drawing"]["busy"] and bool(d["drawing"]["predictions"]) and all(not p["detexify"] for p in d["drawing"]["predictions"]), timeout=30)
        assert state["document"]["drawing"]["strokes"] == strokes
        prediction = state["document"]["drawing"]["predictions"][0]
        assert not prediction["typst"].startswith("#sym."), "Unicode preference should apply"
        self.click("drawing.prediction." + str(prediction["index"]))
        self.source_is(prediction["typst"])
        self.settings()
        self.setting_section("handwriting", "Handwriting recognition")
        self.click("settings.handwriting.Detexify")
        self.click("settings.handwriting.names")
        self.close_settings()
        self.click("drawing.clear")
        self.wait("Clear discards ink and old predictions", lambda d: not d["drawing"]["strokes"] and not d["drawing"]["predictions"] and not d["drawing"]["busy"])
        self.click("explorer.maximize.Draw symbol")
        self.close_scratch(count)

    def completion(self):
        count = self.scratch("#mi(`\\alp`)")
        self.key(123, "none")
        self.key(123, "none")
        self.key(49, "ctrl")  # Explicit completion request.
        self.wait("completion popup receives a real response", lambda d: d["completion_visible"] and d["completion_count"] > 0, timeout=30)
        self.wait("embedded TeX completion has a built-in provider symbol", lambda d: set(d["completion_origins"]) == {"Built-in completion"})
        self.wait_target("completion.item.0")
        if self.capture_review:
            self.capture_viewport("main")
        self.key(53, "none")  # Escape dismisses without editing the source.
        self.wait("completion dismisses without changing source", lambda d: not d["completion_visible"])
        self.source_is("#mi(`\\alp`)")
        self.close_scratch(count)
        count = self.scratch("#rect")
        self.wait("real Tinymist editor session is ready", lambda d: d["editor_lsp_ready"], timeout=30)
        self.key(49, "ctrl")
        self.wait("Tinymist completion has a language-server provider symbol", lambda d: d["completion_visible"] and "Language server" in d["completion_origins"], timeout=30)
        if self.capture_review:
            self.capture_viewport("main")
        self.key(53, "none")
        self.source_is("#rect")
        self.close_scratch(count)

        count = self.scratch('#set text(font: "Times")')
        self.key(123, "none")
        self.key(123, "none")
        self.key(49, "ctrl")
        state = self.wait("installed-font names are rendered in their actual font", lambda d:
                          d["completion_visible"] and set(d["completion_origins"]) == {"Installed font"}
                          and d["completion_font_name_ready"], timeout=30)
        name = state["document"]["completion_font_name"]
        self.wait_target("completion.item.0")
        if self.capture_review:
            self.capture_viewport("main")
        self.key(36, "none")
        escaped = name.replace("\\", "\\\\").replace('"', '\\"')
        self.source_is('#set text(font: "' + escaped + '")')
        self.close_scratch(count)

    def explorer_toggle(self):
        """Native Cmd+1 must close atomically, with a stable independent edge."""
        from concurrent.futures import ThreadPoolExecutor

        def toggle(expected_visible, independent, preview_edge):
            sampled = set()

            def inspect(state):
                sampled.add(assert_explorer_frame(state, independent, preview_edge))

            # Observe while the native driver posts key-down/up, then inspect
            # every returned frame until the requested transition has settled.
            with ThreadPoolExecutor(max_workers=1) as worker:
                command = worker.submit(self.native, "key", 18, "cmd")
                while not command.done():
                    inspect(self.snapshot())
                command.result()
            state = self.wait("Cmd+1 toggles Explorer without freezing", lambda d:
                              d["explorer_visible"] == expected_visible and d["source_fingerprint"] == source)
            inspect(state)
            for _ in range(8):
                state = self.snapshot()
                inspect(state)
                if state["document"]["explorer_visible"] != expected_visible:
                    raise AssertionError("Explorer changed again after the requested toggle")
            self.record("explorer_toggle.frames", visible=expected_visible,
                        independent=independent, sampled=len(sampled))
        self.ensure_typesetting_tab()
        self.key(20)
        initial = self.wait("split layout ready", lambda d: d["view_mode"] == "Split")["document"]
        source = initial["source_fingerprint"]
        for backend in ("Interactive", "Pdfium"):
            self.choose_backend(backend)
            for independent in (False, True):
                self.settings()
                self.setting_section("independent main dividers", "Independent main dividers")
                if self.snapshot()["document"]["independent_main_dividers"] != independent:
                    self.click("settings.dividers")
                self.wait("divider policy applied", lambda d: d["independent_main_dividers"] == independent)
                self.close_settings()
                for after_resize in (False, True):
                    if after_resize:
                        self.wait_target("divider.explorer")
                        before = self.snapshot()
                        border = before["targets"]["divider.explorer"]
                        preview_edge = before["targets"]["divider.editor"]["x"]
                        code_width = before["document"]["editor_panel_width"]
                        self.native("drag", border["x"], border["y"], border["x"] + 35, border["y"])
                        self.wait("divider drag finishes without blocking UI", lambda d:
                                  abs(d["explorer_panel_width"] - before["document"]["explorer_panel_width"] - 35) < 2)
                        state = self.snapshot()
                        if independent:
                            if abs(state["targets"]["divider.editor"]["x"] - preview_edge) > 1:
                                raise AssertionError("independent drag changed preview edge")
                        elif abs(state["document"]["editor_panel_width"] - code_width) > 1:
                            raise AssertionError("fixed-width drag changed editor width")
                    for _ in range(3):
                        preview_edge = self.snapshot()["targets"]["divider.editor"]["x"]
                        toggle(False, independent, preview_edge)
                        toggle(True, independent, preview_edge)
                if independent:
                    # The split pane is retained while Code/Preview-only is
                    # shown. Moving Explorer there must not reset its anchor.
                    for key, mode in ((19, "Code"), (21, "Preview")):
                        edge = self.snapshot()["targets"]["divider.editor"]["x"]
                        files = self.snapshot()["scrolls"]["Files"]
                        self.native("click", files["x"], files["y"])
                        self.wait("Files is focused before view command", lambda d:
                                  d["explorer_focused"] == "Files")
                        self.key(key)
                        self.wait("split editor is hidden in " + mode, lambda d:
                                  d["view_mode"] == mode)
                        if mode == "Code":
                            self.wait("Code command retires Explorer keyboard focus", lambda d:
                                      d["editor_keyboard_focused"] and d["explorer_focused"] is None)
                            self.key(3)
                            self.wait("Find follows explicit source focus", lambda d: d["find_visible"])
                            self.click("find.close")
                            files = self.snapshot()["scrolls"]["Files"]
                            self.native("click", files["x"], files["y"])
                            self.wait("Files owns focus before closing Explorer", lambda d:
                                      d["explorer_focused"] == "Files")
                        self.key(18)
                        self.wait("Explorer closes while split is hidden", lambda d:
                                  not d["explorer_visible"] and d["explorer_focused"] is None)
                        if mode == "Code":
                            self.key(3)
                            self.wait("hidden Explorer cannot swallow Find", lambda d: d["find_visible"])
                            self.click("find.close")
                        self.key(20)
                        self.wait("Split returns after Explorer closes", lambda d: d["view_mode"] == "Split")
                        self.wait_target("divider.editor")
                        assert_explorer_frame(self.snapshot(), True, edge)
                        self.key(key)
                        self.wait("split hides again in " + mode, lambda d: d["view_mode"] == mode)
                        self.key(18)
                        self.wait("Explorer reopens while split is hidden", lambda d: d["explorer_visible"])
                        self.wait_target("divider.explorer")
                        before = self.snapshot()
                        border = before["targets"]["divider.explorer"]
                        width = before["document"]["explorer_panel_width"]
                        self.native("drag", border["x"], border["y"], border["x"] + 20, border["y"])
                        self.wait("Explorer resizes while split is hidden", lambda d:
                                  abs(d["explorer_panel_width"] - width - 20) < 2)
                        if mode == "Preview" and backend == "Interactive":
                            if self.capture_review:
                                self.capture_viewport("main")
                            self.review_native_preview("Explorer-preview")
                        self.key(20)
                        self.wait("Split returns after hidden Explorer resize", lambda d: d["view_mode"] == "Split")
                        self.wait_target("divider.editor")
                        for _ in range(8):
                            assert_explorer_frame(self.snapshot(), True, edge)
                        self.record("explorer_toggle.hidden_split", mode=mode, backend=backend)
                self.click("editor.source")
                self.key(125, "cmd")
                self.native("text", "x")
                self.wait("editor typing still works after layout transitions", lambda d:
                          d["editor_keyboard_focused"] and d["source_fingerprint"] != source)
                self.key(6)
                self.wait("undo preserves document after layout transitions", lambda d: d["source_fingerprint"] == source)

        # A nearly full-height bottom panel leaves a shorter vertical divider.
        # Its paint-only grip must fit that span instead of leaking into the toolbar.
        self.key(23)
        self.wait("bottom panel opens for short-divider coverage", lambda d: d["panel"] is not None)
        self.click("panel.activity")
        self.wait_target("divider.bottom")
        state = self.snapshot()
        bottom = state["targets"]["divider.bottom"]
        editor = state["targets"]["divider.editor"]
        # This is the real resize widget, intersected with the bottom panel;
        # its center is clear of the source editor on the shared boundary.
        self.native("move", bottom["x"], bottom["y"])
        self.wait("real bottom resize handle is hovered", lambda _: self.snapshot()["targets"]["divider.bottom"]["hovered"])
        self.native("drag", bottom["x"], bottom["y"], bottom["x"],
                    editor["y"] - editor["height"] / 2 + 12)
        self.wait("nearly maximized bottom panel leaves a short editor span", lambda d:
                  d["panel"] == "Activity" and not d["panel_maximized"])
        self.wait_target("divider.editor")
        short = self.snapshot()["targets"]["divider.editor"]
        # Child widgets may demand extra height, but the observed/painted
        # grip must use the twelve-point visible main-panel allocation.
        if not 0 < short["height"] < 24:
            raise AssertionError(f"near-maximized panel did not shorten the divider: {short['height']}")
        self.stable_state(lambda d: d["source_fingerprint"] == source)
        if self.capture_review:
            self.capture_viewport("main")
        self.click("panel.close")
        self.wait("short-divider fixture restores normal editor", lambda d: d["panel"] is None)

    def layout(self):
        # The dialog journey intentionally leaves an arbitrary-extension tab
        # active. View-mode and panel commands are document-scoped, so make
        # this journey self-contained instead of relying on aggregate order.
        self.ensure_typesetting_tab()
        initial = self.snapshot()["document"]
        for key, mode in ((19, "Code"), (21, "Preview"), (20, "Split")):
            self.key(key)
            self.wait("view mode " + mode, lambda d: d["view_mode"] == mode)
        # The compact Layout picker uses the real native popup above a live
        # Tinymist child; exercise every row rather than only its shortcuts.
        self.choose_backend("Interactive", save_pdf=False)
        self.wait("Layout picker has a live native preview behind it", lambda d:
                  d["preview_backend"] == "Tinymist" and d["preview_native_ready"], timeout=40)
        self.click("toolbar.layout")
        self.wait_target("layout.code")
        self.key(53, "none")
        self.wait("Escape returns Layout focus to its document", lambda d: d["focused"])
        for mode in ("Code", "Preview", "Split"):
            self.click("toolbar.layout")
            if mode == "Code":
                if self.capture_review:
                    self.capture_viewport("popup")
                self.review_native_preview("layout-native-popup", document_focus=False)
            self.click("layout." + mode.lower())
            self.wait("Layout picker selects " + mode, lambda d: d["view_mode"] == mode)
        self.click("toolbar.layout")
        self.click("layout.explorer")
        self.wait("Layout picker hides Explorer", lambda d: not d["explorer_visible"])
        self.key(18)
        self.wait("Explorer shortcut still restores the same sections", lambda d: d["explorer_visible"])
        for section in ("Tags", "Files"):
            self.wait_target("explorer.maximize." + section)
            self.click("explorer.maximize." + section)
            self.wait("explorer section maximized", lambda d: d["explorer_maximized"] == section)
            for _ in range(3):
                state = self.snapshot()
                visible = {name for name, target in state["targets"].items()
                           if name.startswith("explorer.maximize.") and target["frame"] == state["document"]["frame"]}
                if visible != {"explorer.maximize." + section}:
                    raise AssertionError(f"maximized section left sibling controls visible: {visible}")
            self.click("explorer.maximize." + section)
            self.wait("explorer sections restored", lambda d: d["explorer_maximized"] is None)
        for code, field in ((6, "line_wrap"), (45, "line_numbers"), (17, "sticky_context")):
            self.key(code, "cmd+alt")
            self.wait("editor setting toggled", lambda d: d[field] != initial[field])
            self.key(code, "cmd+alt")
            self.wait("editor setting restored", lambda d: d[field] == initial[field])
        self.key(23)
        document = self.wait("panel opens", lambda d: d["panel"] is not None)["document"]
        baseline = document["panel_rect"][3] - document["panel_rect"][1]
        self.click("panel.maximize")
        self.wait("maximize button", lambda d: d["panel_maximized"] and d["panel_rect"][3] - d["panel_rect"][1] > baseline)
        self.key(23, "cmd+alt")
        self.wait("maximize shortcut restores", lambda d: not d["panel_maximized"])
        self.stable(baseline)
        self.click("panel.activity")
        self.wait("Activity selected", lambda d: d["panel"] == "Activity")
        self.stable(baseline)
        self.click("toolbar.layout")
        self.click("layout.panel")
        self.wait("Layout picker hides the bottom panel", lambda d: d["panel"] is None)
        self.click("toolbar.layout")
        self.click("layout.panel")
        self.wait("Layout picker restores the selected bottom tab", lambda d: d["panel"] == "Activity")
        self.stable(baseline)
        self.click("panel.close")
        self.wait("close panel button", lambda d: d["panel"] is None)
        # The Appearance setting restores the original one-click controls.
        self.settings()
        self.setting_section("compact layout", "Compact layout controls")
        self.click("settings.compact_layout")
        self.wait("separate layout buttons selected", lambda d: not d["compact_layout_controls"])
        self.close_settings()
        if self.capture_review:
            self.capture_viewport("main")
        for mode in ("Code", "Preview", "Split"):
            self.click("toolbar.layout." + mode.lower())
            self.wait("separate layout button selects " + mode, lambda d: d["view_mode"] == mode)
        self.click("toolbar.layout.explorer")
        self.wait("separate Explorer button hides panel", lambda d: not d["explorer_visible"])
        self.click("toolbar.layout.explorer")
        self.wait("separate Explorer button restores panel", lambda d: d["explorer_visible"])
        self.click("toolbar.layout.panel")
        self.wait("separate Panel button restores selected tab", lambda d: d["panel"] == "Activity")
        self.stable(baseline)
        self.click("toolbar.layout.panel")
        self.wait("separate Panel button hides panel", lambda d: d["panel"] is None)
        self.settings()
        self.setting_section("compact layout", "Compact layout controls")
        self.click("settings.compact_layout")
        self.wait("compact Layout restored", lambda d: d["compact_layout_controls"])
        self.close_settings()
        self.wait_target("toolbar.layout")
        self.wait("layout leaves source unchanged", lambda d: d["source_fingerprint"] == initial["source_fingerprint"])

    def explorer_scroll(self):
        self.ensure_typesetting_tab()
        import time
        def scroll_stays(section, sticky=False):
            deadline = time.monotonic() + 10
            while True:
                state = self.snapshot()
                scroll = state["scrolls"].get(section)
                if (scroll and scroll["frame"] == state["document"]["frame"]
                        and scroll["content_height"] > scroll["viewport_height"] + 100
                        and scroll["sticky_search"] == sticky):
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError(f"{section} did not expose a scrollable list")
                time.sleep(0.05)
            before = scroll["offset"]
            self.native("move", scroll["x"], scroll["y"])
            self.native("wheel", scroll["x"], scroll["y"], -8)
            deadline = time.monotonic() + 3
            while True:
                after = self.snapshot()["scrolls"][section]["offset"]
                if after > before + 20:
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError(f"{section} did not scroll after wheel input: {before} -> {after}")
                time.sleep(0.05)
            for _ in range(8):
                after = self.snapshot()["scrolls"][section]["offset"]
                if after <= before + 20:
                    raise AssertionError(f"{section} scroll reset after wheel input: {before} -> {after}")
            self.record("explorer.scroll.stable", section=section, sticky=sticky, before=before, after=after)

        scroll_stays("Files")
        if self.explorer_scroll_checks_contents:
            scroll_stays("Contents")
            files = self.snapshot()["scrolls"]["Files"]
            self.native("click", files["x"], files["y"])
            self.wait("Files receives focus", lambda d: d["explorer_focused"] == "Files")
            self.key(3)  # Cmd+F exposes the sticky Files search.
            scroll_stays("Files", sticky=True)
            if getattr(self, "capture_review", False):
                self.capture_viewport("main")

    def folding(self):
        text = "#let block = {\n  let a = 1\n  a + 2\n}\n\nEnd."
        count = self.scratch(text)
        self.key(126, "cmd")
        self.wait("fold header available", lambda d: len(d["fold_headers"]) > 0 and d["cursor"] == [0, 0])
        headers = self.snapshot()["document"]["fold_headers"]
        for _ in range(3):
            self.key(37, "cmd+alt")
            self.wait("fold collapses", lambda d: len(d["fold_collapsed"]) == 1 and d["fold_headers"] == headers)
            self.stable_state(lambda d: len(d["fold_collapsed"]) == 1 and d["fold_headers"] == headers)
            self.key(37, "cmd+alt")
            self.wait("fold expands", lambda d: not d["fold_collapsed"] and d["fold_headers"] == headers)
        self.key(125, "cmd")
        self.wait("caret at document end", lambda d: d["cursor"] == [len(text), len(text)])
        target = "fold." + str(headers[0])
        self.wait_target(target)
        self.click(target)
        self.wait("gutter preserves visible caret", lambda d: len(d["fold_collapsed"]) == 1 and d["fold_headers"] == headers and d["cursor"] == [len(text), len(text)])
        self.wait_target(target)
        self.click(target)
        self.wait("same gutter control expands", lambda d: not d["fold_collapsed"] and d["fold_headers"] == headers)
        self.source_is(text)
        self.close_scratch(count)
        self.folding_large()

    def folding_large(self):
        prefix = "\n".join(f"Prose line {i}." for i in range(8)) + "\n"
        block = "#let large = {\n" + "\n".join(f"  let value{i} = {i}" for i in range(180)) + "\n}\n"
        suffix = "\n".join(f"Following prose {i}." for i in range(100))
        text = prefix + block + suffix
        count = self.scratch(text)
        self.key(125, "cmd")
        self.wait("large document caret at end", lambda d: d["cursor"] == [len(text), len(text)])
        self.stable_state(lambda d: d["cursor"] == [len(text), len(text)])
        target = "fold.8"
        def scroll_to_header():
            for _ in range(30):
                state = self.snapshot()
                scroll = state["scrolls"]["editor"]
                if scroll["offset"] < 1:
                    break
                self.native("move", scroll["x"], scroll["y"])
                self.native("wheel", scroll["x"], scroll["y"], 100)
            self.wait_target(target)
        scroll_to_header()
        self.click(target)
        self.wait("large fold preserves distant caret", lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [len(text), len(text)])
        self.stable_state(lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [len(text), len(text)])
        assert self.snapshot()["scrolls"]["editor"]["offset"] < 1, "fold must not scroll to the distant caret"
        self.click(target)
        self.wait("large block expands without moving caret", lambda d: not d["fold_collapsed"] and d["cursor"] == [len(text), len(text)])
        # Put the caret inside the block through ordinary keyboard navigation.
        self.click("editor.source")
        self.key(126, "cmd")
        for _ in range(18): self.key(125, "none")
        inside = self.snapshot()["document"]["cursor"][0]
        assert len(prefix) < inside < len(prefix + block)
        scroll_to_header()
        self.click(target)
        header_end = len(prefix) + len("#let large = {")
        self.wait("hidden caret moves to visible fold header", lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [header_end, header_end])
        self.stable_state(lambda d: 8 in d["fold_collapsed"] and d["cursor"] == [header_end, header_end])
        self.source_is(text)
        self.close_scratch(count)

    def selection_wrap(self):
        text = "α🦀 words"
        count = self.scratch(text)
        for opening, closing in (("(", ")"), ("[", "]"), ("{", "}"), ("$", "$"), ('"', '"'), ("*", "*"), ("_", "_"), ("`", "`")):
            self.key(0)
            self.native("text", opening)
            self.source_is(opening + text + closing)
            self.key(6)
            self.source_is(text)
            self.key(6, "cmd+shift")
            self.source_is(opening + text + closing)
            self.key(6)
            self.source_is(text)
        self.close_scratch(count)

    def git_hunks(self):
        with self.exact_math_conversion():
            self._git_hunks()

    def _git_hunks(self):
        source = self.directory / "hunks.typ"
        if self.snapshot()["document"]["path"] != str(source):
            import time
            self.key(31)
            time.sleep(0.8)
            self.native("ax-press-description", "search")
            self.native("text", source.name)
            time.sleep(0.8)
            self.native("ax-press-title", "Open")
            self.wait("hunk fixture opened", lambda d: d["path"] == str(source), timeout=15)
        lines = source.read_text().splitlines(keepends=True)
        original = "".join(lines)
        self.choose_backend("Pdfium")
        self.key(20)  # Split: the docked editor must preserve this resizable pane.
        self.wait("split editor shown", lambda d: d["view_mode"] == "Split" and d["editor_panel_width"] is not None)
        self.wait("three separated Git hunks", lambda d: d["git_hunk_count"] == 3)
        self.click("editor.source")
        self.key(126, "cmd")
        initial = self.wait("overview ready at the start of the large document", lambda d:
                            d["cursor"] == [0, 0] and isinstance(d.get("git_overview"), dict))
        overview = assert_git_overview(initial["document"])
        pane_width = initial["document"]["editor_panel_width"]
        scroll = initial["scrolls"].get("editor")
        if (not scroll or scroll["frame"] != initial["document"]["frame"]
                or scroll["content_height"] <= scroll["viewport_height"] + 100):
            raise AssertionError("Git overview fixture did not expose a fresh scrollable code pane")
        last_hunk = initial["targets"].get("git.hunk.2")
        if last_hunk and last_hunk["frame"] == initial["document"]["frame"]:
            raise AssertionError("last fixture hunk must be offscreen when its overview tick is checked")
        self.native("move", scroll["x"], scroll["y"])
        self.native("wheel", scroll["x"], scroll["y"], -8)
        import time
        deadline = time.monotonic() + 3
        while True:
            moved = self.snapshot()
            current_scroll = moved["scrolls"].get("editor")
            if (current_scroll and current_scroll["frame"] == moved["document"]["frame"]
                    and current_scroll["offset"] > scroll["offset"] + 20):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("normal native wheel input did not scroll the code pane")
            time.sleep(0.05)
        for _ in range(8):
            moved = self.snapshot()
            current_scroll = moved["scrolls"].get("editor")
            if (not current_scroll or current_scroll["frame"] != moved["document"]["frame"]
                    or current_scroll["offset"] <= scroll["offset"] + 20):
                raise AssertionError("code pane scroll did not persist across fresh frames")
            current = assert_git_overview(moved["document"])
            if (abs(moved["document"]["editor_panel_width"] - pane_width) > 0.5
                    or any(abs(a - b) > 0.5 for key in ("viewport", "track")
                           for a, b in zip(overview[key], current[key]))
                    or any(abs(a - b) > 0.5 for before, after in zip(overview["ticks"], current["ticks"])
                           for a, b in zip(before, after))):
                raise AssertionError("Git overview moved with the scroll or changed the code pane width")
        self.source_is(original)
        self.record("git.overview.scroll-stable", before=overview, after=current,
                    scroll_before=scroll["offset"], scroll_after=current_scroll["offset"], pane_width=pane_width)
        self.settings()
        self.setting_section("independent main dividers", "Independent main dividers")
        self.click("settings.dividers")
        self.wait("independent dividers enabled", lambda d: d["independent_main_dividers"])
        self.close_settings()
        self.wait_target("divider.explorer")
        state = self.snapshot()
        border = state["targets"]["divider.explorer"]
        preview_edge = state["targets"]["divider.editor"]["x"]
        from concurrent.futures import ThreadPoolExecutor
        moving_frames = 0
        with ThreadPoolExecutor(max_workers=1) as worker:
            drag = worker.submit(self.native, "drag", border["x"], border["y"], border["x"] + 35, border["y"])
            while not drag.done():
                frame = self.snapshot()
                if abs(frame["targets"]["divider.explorer"]["x"] - border["x"]) > 0.5:
                    moving_frames += 1
                if abs(frame["targets"]["divider.editor"]["x"] - preview_edge) > 1:
                    raise AssertionError("preview border moved during independent divider drag")
            drag.result()
        self.wait("Explorer divider moved", lambda d: abs(d["explorer_panel_width"] - state["document"]["explorer_panel_width"] - 35) < 2)
        self.record("independent_resize.frames", intermediate=moving_frames)
        moved = self.snapshot()
        if abs(moved["targets"]["divider.editor"]["x"] - preview_edge) > 1:
            raise AssertionError("independent Explorer resize moved preview border")
        if abs(moved["targets"]["divider.explorer"]["x"] - border["x"] - 35) > 2:
            raise AssertionError("Explorer divider did not follow the drag")
        self.settings()
        self.setting_section("independent main dividers", "Independent main dividers")
        self.click("settings.dividers")
        self.wait("fixed code width mode enabled", lambda d: not d["independent_main_dividers"])
        self.close_settings()
        self.wait_target("divider.explorer")
        state = self.snapshot()
        border = state["targets"]["divider.explorer"]
        width = state["document"]["editor_panel_width"]
        self.native("drag", border["x"], border["y"], border["x"] - 35, border["y"])
        self.stable_state(lambda d: abs(d["editor_panel_width"] - width) < 1)

        self.click("editor.source")
        self.key(126, "cmd")
        for line in (5, 85, 165, 5):
            self.key(125, "cmd+alt")
            self.wait("keyboard hunk jump keeps normal editor focused", lambda d: d["editor_keyboard_focused"]
                      and not d["git_diff_open"] and d["cursor"][0] == sum(map(len, lines[:line])))
        # A keyboard jump can publish its cursor before its scroll settles.
        # Track the fresh gutter position while hovering, as click() does for
        # moving hit targets, rather than hovering an old offscreen rectangle.
        self.wait_target("git.hunk.0")
        deadline = time.monotonic() + 6
        while True:
            state = self.snapshot()
            target = state["targets"].get("git.hunk.0")
            if target and target["frame"] == state["document"]["frame"]:
                self.native("move", target["x"], target["y"])
            state = self.snapshot()["document"]
            if state["hover_tooltip_open"] and not state["git_diff_open"] and state["editor_keyboard_focused"]:
                break
            if time.monotonic() >= deadline:
                raise AssertionError("read-only hunk hover did not preserve editor focus")
            time.sleep(0.05)
        self.click("git.hunk.0")
        self.wait("full document diff opens at clicked change", lambda d: d["git_diff_open"] and d["git_diff_lines"] == [180, 180]
                  and d["cursor"][0] == sum(map(len, lines[:5])))
        assert_git_overview(self.snapshot()["document"])
        # The diff is the same editor: its context menu must preserve selected
        # text for Copy and offer the direct Typst math conversion as well.
        import subprocess
        import time
        self.wait_target("editor.caret")
        caret = self.snapshot()["targets"]["editor.caret"]
        start = sum(map(len, lines[:5]))
        end = start + len(lines[5].rstrip("\n"))
        self.key(124, "cmd+shift")
        self.wait("docked editor selects the changed line", lambda d: sorted(d["cursor"]) == [start, end])
        self.right_click_at(caret["x"], caret["y"])
        self.wait_target("menu.Copy")
        self.wait("docked context menu preserves selected text", lambda d: d["git_diff_open"]
                  and sorted(d["cursor"]) == [start, end])
        self.copy_from_menu(lines[5].rstrip("\n"))
        self.wait("Copy keeps the docked editor focused", lambda d: d["git_diff_open"] and d["editor_keyboard_focused"])
        self.key(126, "cmd")
        prefixed = "$x$ " + original
        self.paste_source("$x$ ", check=lambda: self.source_is(prefixed))
        self.key(126, "cmd")
        self.key(124, "none")
        self.wait_target("editor.caret")
        caret = self.snapshot()["targets"]["editor.caret"]
        self.key(124, "shift")
        self.wait("docked editor selects the formula", lambda d: sorted(d["cursor"]) == [1, 2])
        self.right_click_at(caret["x"], caret["y"])
        self.wait_target("menu.Convert to display math")
        self.click("menu.Convert to display math")
        self.source_is("$\nx\n$ " + original)
        self.wait("conversion keeps the full diff open", lambda d: d["git_diff_open"])
        self.key(6)
        self.source_is(prefixed)
        self.key(6)
        self.source_is(original)
        self.key(126, "cmd")
        self.key(125, "cmd+alt")
        self.wait("hunk navigation resumes after menu actions", lambda d: d["cursor"][0] == start and d["git_diff_open"])
        for button, line in (("Next",85), ("Next",165), ("Next",5), ("Previous",165)):
            self.click("git.hunk." + button)
            arrived = self.wait("navigation moves live editor cursor", lambda d: d["git_diff_open"] and d["cursor"][0] == sum(map(len, lines[:line])) and d["editor_keyboard_focused"])
            if button == "Next" and line == 165:
                at = sum(map(len, lines[:line]))
                self.jump_reveal_feedback("Diff", "source", arrived, region=(line, at, at + len(lines[line])))
            self.stable_state(lambda d: d["git_diff_open"] and d["cursor"][0] == sum(map(len, lines[:line])))
        self.native("resize-focused", 1400, 850)
        self.wait("document diff resizes with owner", lambda d: d["native_size"] == [1400, 850] and d["git_diff_open"])
        self.click("git.hunk.Next")  # Wrap to first changed line.
        self.wait("first change selected", lambda d: d["cursor"][0] == sum(map(len, lines[:5])))
        if getattr(self, "capture_review", False): self.capture_viewport("main")
        pair = self.snapshot()["document"]["git_diff_changed_pairs"][0]
        self.click("git.hunk.line." + str(pair))
        expected = original.replace("005 changed", "005 original")
        self.source_is(expected)
        self.wait("line revert immediately refreshes live diff", lambda d: d["git_diff_open"] and len(d["git_diff_hunks"]) == 2)
        self.key(6)
        self.source_is(original)
        self.wait("undo restores live highlights", lambda d: len(d["git_diff_hunks"]) == 3)
        builds = self.snapshot()["document"]["git_diff_builds"]
        self.key(126, "cmd")
        self.native("text", "Prefix ")
        self.source_is("Prefix " + original)
        self.wait("editing outside original hunk refreshes full diff", lambda d: d["git_diff_open"] and d["git_diff_changed_pairs"] == [0, 5, 85, 165] and d["git_diff_hunks"] == [0, 85, 165] and d["git_diff_builds"] > builds)
        self.key(6)
        self.source_is(original)
        self.key(6, "cmd+shift")
        self.source_is("Prefix " + original)
        self.key(6)
        self.source_is(original)
        self.key(125, "cmd")
        self.native("text", "Extra line")
        self.source_is(original + "Extra line")
        self.wait("last line is editable and not hidden", lambda d: d["git_diff_lines"] == [180, 181])
        if getattr(self, "capture_review", False): self.capture_viewport("main")
        self.key(6)
        self.source_is(original)
        width = self.snapshot()["document"]["editor_panel_width"]
        self.key(53, "none")
        self.wait("Escape returns to code without resizing pane", lambda d: not d["git_diff_open"] and d["editor_keyboard_focused"] and abs(d["editor_panel_width"] - width) < 1)
        self.stable_state(lambda d: abs(d["editor_panel_width"] - width) < 1)
        self.wait("original gutter markers refreshed", lambda d: d["git_hunk_count"] == 3)
        self.key(126, "cmd")
        self.wait("source cursor returned to start", lambda d: d["cursor"] == [0, 0])
        self.wait_target("git.file.hunks.typ")
        before = self.snapshot()["document"]
        self.double_click("git.file.hunks.typ")
        reopened = self.wait("Git file double-click opens a docked whole-document diff", lambda d:
                            d["git_diff_open"] and d["git_diff_lines"] == [180, 180]
                            and len(d["git_diff_hunks"]) == 3
                            and d["source_fingerprint"] == before["source_fingerprint"]
                            and abs(d["editor_panel_width"] - before["editor_panel_width"]) < 1)
        assert_git_overview(reopened["document"])
        self.stable_state(lambda d: d["git_diff_open"]
                          and d["source_fingerprint"] == before["source_fingerprint"]
                          and abs(d["editor_panel_width"] - before["editor_panel_width"]) < 1)
        self.source_is(original)
        self.key(3)
        self.wait("Find owns keyboard while document diff stays open", lambda d: d["find_focused"] and d["git_diff_open"])
        self.native("text", "Line 085")
        self.wait("Find query updates without stealing focus", lambda d: d["find_focused"] and d["find_query_fingerprint"] == fingerprint("Line 085") and d["find_selected"] is not None)
        self.source_is(original)
        self.key(3)
        self.wait("closing Find preserves document diff", lambda d: not d["find_visible"] and d["git_diff_open"] and d["editor_keyboard_focused"])
        self.choose_backend("Interactive")
        self.wait_target("git.hunk.Next")
        target = self.snapshot()["targets"]["git.hunk.Next"]
        self.native("move", target["x"], target["y"])
        self.wait("docked diff tooltip gets its own native window above preview", lambda d: d["git_diff_open"] and d["hover_tooltip_open"] and d["native_hover_window"])
        if getattr(self, "capture_review", False): self.capture_viewport("main")
        # Action shortcuts must target the live caret, not the initially opened hunk.
        self.click("git.hunk.editor")
        self.key(126, "cmd")
        self.key(125, "cmd+alt")
        self.key(125, "cmd+alt")
        at = sum(len(line) for line in original.splitlines(keepends=True)[:85])
        self.wait("diff cursor reaches second hunk", lambda d: d["cursor"] == [at, at])
        self.key(1, "cmd+alt")  # Stage the second hunk.
        import subprocess
        expected_index = original.replace("005 changed", "005 original").replace("165 changed", "165 original")
        self.wait("keyboard stages only the hunk under the diff caret", lambda d:
            subprocess.run(["git", "-C", str(self.directory), "show", ":hunks.typ"],
                           capture_output=True, text=True, check=True).stdout == expected_index)
        self.key(32, "cmd+alt")  # Unstage the same hunk (U).
        committed = expected_index.replace("085 changed", "085 original")
        self.wait("keyboard unstages the caret hunk", lambda d:
            subprocess.run(["git", "-C", str(self.directory), "show", ":hunks.typ"],
                           capture_output=True, text=True, check=True).stdout == committed)


    def pdfium_resize(self):
        """Drag real dividers repeatedly while retaining the last sharp textures."""
        import time
        self.focus_document_window()
        before = self.snapshot()["document"]
        fixture = self.directory / "pdfium-resize.tex"
        # TeX always uses PDFium, independent of the Typst backend preference.
        # Tall pages keep demand constant, isolating resizing from new pages.
        source = r'\documentclass{article}' + '\n'
        source += r'\usepackage[paperwidth=400pt,paperheight=1600pt,margin=24pt]{geometry}' + '\n'
        source += r'\begin{document}' + '\n'
        for page in range(3):
            if page:
                source += r'\newpage' + '\n'
            source += r'\section*{Resize page ' + str(page + 1) + '}\n'
            for row in range(12):
                source += ('PDFium resident row ' + str(row + 1)
                           + r'. \( \int_0^1 x^2\,dx = 1/3 \).\par' + '\n')
        source += r'\end{document}' + '\n'
        fixture.write_text(source)
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(fixture))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        self.wait("isolated PDFium resize document opened", lambda d: d["path"] == str(fixture), timeout=15)
        fixture_tab = self.snapshot()["document"]["active_tab_id"]
        self.key(35, "cmd+alt")
        self.wait("resize fixture is designated for preview", lambda d:
                  d["preview_tab_id"] == fixture_tab and d["preview_path"] == str(fixture))
        self.key(20)
        self.key(15)
        self.wait("TeX compiles into the effective PDFium resize preview", lambda d:
                  pdfium_resize_settled(d) and d["preview_pages"] == 3
                  and not d["compile_indicator_spinning"], timeout=60)
        if not self.snapshot()["document"]["preview_controls_open"]:
            self.click("preview.open")
        self.wait_target("preview.Fit page width")
        self.click("preview.Fit page width")
        self.click("preview.Minimize controls")
        self.wait("fit-width PDFium ready before divider drags", pdfium_resize_settled, timeout=20)
        self.stable_state(pdfium_resize_settled)
        for iteration, delta in enumerate((96, -96, 96, -96)):
            self.wait_target("divider.editor")
            state = self.snapshot()
            handle = state["targets"]["divider.editor"]
            baseline = pdfium_render_state(state["document"])
            x, y = handle["x"], handle["y"]
            last_x = x
            snapshot_ms = []
            started = time.monotonic()
            self.native("drag-start", x, y)
            try:
                for step in range(1, 7):
                    last_x = x + delta * step / 6
                    self.native("drag-move", last_x, y)
                    observed_at = time.monotonic()
                    frame = self.snapshot()
                    snapshot_ms.append((time.monotonic() - observed_at) * 1000)
                    assert_pdfium_resize_held(frame["document"], baseline)
                    if frame["document"]["source_fingerprint"] != fingerprint(source):
                        raise AssertionError("resizing PDFium edited the source")
                    if step == 3:
                        # Outlast the quiet deadline with the mouse still held.
                        time.sleep(0.150)
                        assert_pdfium_resize_held(self.snapshot()["document"], baseline)
                        if self.capture_review and iteration == 0:
                            self.capture_viewport("main")
                frame = self.snapshot()
                moved = frame["targets"].get("divider.editor")
                if not moved or moved["frame"] != frame["document"]["frame"] or abs(moved["x"] - last_x) > 2:
                    raise AssertionError("PDFium divider did not track the native drag")
            finally:
                self.native("drag-end", last_x, y)
            final = self.wait("released PDFium divider renders its final physical scale", pdfium_resize_settled,
                              timeout=20)["document"]
            rendered = pdfium_render_state(final)
            if rendered["active_scale"] == baseline["active_scale"]:
                raise AssertionError("PDFium resize never changed the raster scale")
            if rendered["requests"] != baseline["requests"] + 1 or rendered["admissions"] != baseline["admissions"] + 1:
                raise AssertionError("PDFium resize did not publish exactly one final sharp batch")
            self.stable_state(lambda d: pdfium_resize_settled(d)
                              and pdfium_render_state(d)["requests"] == rendered["requests"]
                              and pdfium_render_state(d)["admissions"] == rendered["admissions"])
            self.record("pdfium_resize.drag", iteration=iteration, delta=delta,
                        before=baseline, after=rendered, snapshot_ms=snapshot_ms,
                        elapsed_ms=(time.monotonic() - started) * 1000)
        if self.capture_review:
            self.capture_viewport("main")
        self.restore_partial_fixture_tabs(before, [fixture_tab])
        if before["view_mode"] != "Split":
            self.key(19 if before["view_mode"] == "Code" else 21)
            self.wait("PDFium resize journey restores its original view", lambda d: d["view_mode"] == before["view_mode"])

    def preview_keyboard(self):
        self.ensure_typesetting_tab()
        for backend in ("Pdfium", "Interactive"):
            self.choose_backend(backend)
            self.click("editor.source")
            self.key(125, "cmd")
            before = self.snapshot()["document"]
            self.key(15)  # Compile/reload must preserve the source keyboard owner.
            self.wait("preview ready after compile", lambda d: d["preview_pdfium_ready" if backend == "Pdfium" else "preview_native_ready"])
            target = self.snapshot()["targets"]["preview.page"]
            self.native("move", target["x"], target["y"])
            self.native("wheel", target["x"], target["y"], -3)
            self.stable_state(lambda d: d["editor_keyboard_focused"] and not d["preview_keyboard_focused"])
            self.native("text", "t")
            self.wait("typing stays in source after preview build", lambda d: d["source_fingerprint"] != before["source_fingerprint"] and d["page_dark"] == before["page_dark"])
            self.key(6)
            self.wait("undo restores source", lambda d: d["source_fingerprint"] == before["source_fingerprint"])
            for iteration in range(3):
                self.click("preview.page")
                self.wait("preview owns keyboard after click", lambda d: d["preview_keyboard_focused"] and not d["editor_keyboard_focused"])
                if backend == "Interactive":
                    self.native("text", "t")
                    self.wait("focused preview accepts its shortcut", lambda d: d["page_dark"] != before["page_dark"] and d["source_fingerprint"] == before["source_fingerprint"])
                    self.native("text", "t")
                    self.wait("second inversion restores page", lambda d: d["page_dark"] == before["page_dark"])
                if getattr(self, "capture_review", False) and backend == "Pdfium" and iteration == 0:
                    self.capture_viewport("main")
                self.key(53, "none")
                self.wait("Escape returns to source", lambda d: d["editor_keyboard_focused"] and not d["preview_keyboard_focused"])
                self.native("text", "t")
                self.wait("t edits source instead of inverting preview", lambda d: d["source_fingerprint"] != before["source_fingerprint"] and d["page_dark"] == before["page_dark"])
                self.key(6)
                self.wait("source restored after focus roundtrip", lambda d: d["source_fingerprint"] == before["source_fingerprint"])

            self.click("preview.page")
            self.wait("preview focused before hiding", lambda d: d["preview_keyboard_focused"])
            self.key(19)  # Cmd+2: hiding a focused preview must hand keyboard ownership to source.
            self.wait("code-only view selected", lambda d: d["view_mode"] == "Code")
            self.native("text", "t")
            self.wait("hidden preview cannot retain keyboard input", lambda d: d["source_fingerprint"] != before["source_fingerprint"] and d["page_dark"] == before["page_dark"])
            self.key(6)
            self.key(20)  # Cmd+3 restores split view for the next backend.
            self.wait("split view restored", lambda d: d["view_mode"] == "Split")

    def closing(self):
        text = "Unsaved fixture must survive cancellation."
        count = self.scratch(text)
        self.key(13)
        self.wait("dirty close asks first", lambda d: d["modal_open"] and d["tabs"] == count + 1)
        self.wait_target("modal.cancel")
        self.click("modal.cancel")
        self.wait("Cancel preserves dirty tab", lambda d: not d["modal_open"] and d["dirty"] and d["tabs"] == count + 1)
        self.source_is(text)
        self.key(13)
        self.wait("close asks again", lambda d: d["modal_open"])
        self.key(53, "none")
        self.wait("Escape cancels dirty close", lambda d: not d["modal_open"] and d["tabs"] == count + 1)
        self.source_is(text)
        self.key(13)
        self.wait("explicit discard required", lambda d: d["modal_open"])
        self.wait_target("modal.discard")
        self.click("modal.discard")
        self.wait("discard closes only fixture tab", lambda d: not d["modal_open"] and d["tabs"] == count)

    def choose_backend(self, backend, save_pdf=True):
        self.settings()
        if self.snapshot()["document"]["settings_json"]["visible"]:
            self.click("settings.json.mode")
        self.setting_section("backend", "Typst preview backend")
        self.wait_target("settings.backend." + backend)
        self.click("settings.backend." + backend)
        # Settings is an independently repainted native viewport. Observe the
        # accepted preference before closing it or issuing a compiler command.
        self.wait("selected backend preference applies before Settings closes", lambda d:
                  d["settings_visible"] and d["preview_requested_backend"] == backend)
        self.close_settings()
        if save_pdf:
            self.key(15)
        elif backend in ("Interactive", "InteractivePartial"):
            # Tinymist refresh is preview-only. The CLI gear saves a public PDF,
            # so a PDFium backend-switch check must let the background build run.
            self.wait_target("build.preview")
            generation = self.snapshot()["document"]["preview_generation"]
            self.click("build.preview")
        self.wait("selected renderer ready", lambda d: d["preview_requested_backend"] == backend
                  and d["preview_backend"] == ("PDFium" if backend == "Pdfium" else "Tinymist partial" if backend == "InteractivePartial" else "Tinymist")
                  and d["preview_pdfium_ready" if backend == "Pdfium" else "preview_native_ready"]
                  and (save_pdf or (not d["compile_indicator_spinning"] and
                       (backend not in ("Interactive", "InteractivePartial") or (d["preview_generation"] != "None" and d["preview_generation"] != generation)))), timeout=60)

    def pdf_output(self):
        """Preview activity must never imply an adjacent PDF was saved."""
        import time
        self.focus_document_window()
        before = self.snapshot()["document"]
        if before["find_visible"]:
            self.click("find.close")
            self.wait("PDF journey closes earlier Find", lambda d: not d["find_visible"])
        fixture_directory = self.directory / "pdf-output"
        fixture_directory.mkdir()
        fixture = fixture_directory / "journey.typ"
        fixture.write_text(PREVIEW_SOURCE)
        destination = fixture.with_suffix(".pdf")
        # The aggregate run can already have a journey.pdf from other tests.
        # A new document path isolates output ownership without deleting files.
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(fixture))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        self.wait("isolated three-page Typst document opened", lambda d: d["path"] == str(fixture), timeout=15)
        self.key(35, "cmd+alt")
        self.wait("PDF source is designated for preview", lambda d: d["preview_tab_id"] == d["active_tab_id"] and d["preview_path"] == str(fixture), timeout=30)
        self.key(20)  # Split view so readiness refers to a visible renderer.
        self.tex_settings([('"auto_save": true', '"auto_save": false')])
        self.choose_backend("Interactive", save_pdf=False)
        self.wait("Tinymist preview settles without a disk export", lambda d: d["preview_backend"] == "Tinymist" and d["preview_native_ready"] and d["preview_pages"] == 3 and not d["compile_indicator_spinning"], timeout=60)
        self.stable_state(lambda d: d["pdf_output_state"] == "Unverified" and not d["pdf_output_pending"] and not destination.exists())

        self.wait_target("build.pdf")
        self.click("build.pdf")
        saved = self.wait("PDF button confirms a committed file", lambda d: d["pdf_output_state"] == "Saved" and str(d["pdf_output_revision"]) == d["revision"] and not d["pdf_output_pending"], timeout=60)
        first_pdf = destination.read_bytes()
        if not first_pdf.startswith(b"%PDF-"):
            raise AssertionError("PDF action did not create a PDF document")
        first_mtime = destination.stat().st_mtime_ns
        self.stable_state(lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and destination.read_bytes() == first_pdf and destination.stat().st_mtime_ns == first_mtime, frames=30)
        self.record("pdf.saved", revision=saved["document"]["revision"], bytes=len(first_pdf))
        if getattr(self, "capture_review", False):
            self.capture_viewport("main")

        edited = PREVIEW_SOURCE.replace("Black text on white paper.", "A changed paragraph awaiting PDF export.")
        self.click("editor.source")
        self.key(0)
        self.paste_source(edited)
        self.wait("typing marks the saved PDF stale", lambda d: d["pdf_output_state"] == "Stale" and not d["pdf_output_pending"])
        generation = self.snapshot()["document"]["preview_generation"]
        self.click("build.preview")
        self.wait("updated Tinymist preview settles", lambda d: d["preview_backend"] == "Tinymist" and d["preview_native_ready"] and d["preview_pages"] == 3 and not d["compile_indicator_spinning"] and d["preview_generation"] != "None" and d["preview_generation"] != generation, timeout=60)
        self.stable_state(lambda d: d["pdf_output_state"] == "Stale" and not d["pdf_output_pending"] and destination.read_bytes() == first_pdf and destination.stat().st_mtime_ns == first_mtime)

        self.key(15)  # Cmd+R explicitly saves PDF; engine refresh above did not.
        self.wait("Cmd+R updates the PDF to the new source revision", lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        second_pdf = destination.read_bytes()
        if not second_pdf.startswith(b"%PDF-") or second_pdf == first_pdf:
            raise AssertionError("Cmd+R did not update the saved PDF contents")
        second_mtime = destination.stat().st_mtime_ns
        self.stable_state(lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and destination.read_bytes() == second_pdf and destination.stat().st_mtime_ns == second_mtime, frames=30)
        self.click("editor.source")
        self.key(0)
        self.paste_source("#let broken = {\n")
        self.click("build.pdf")
        self.wait("failed PDF creation is visible and not queued", lambda d: d["pdf_output_state"] == "Failed" and not d["pdf_output_pending"] and not d["compile_indicator_spinning"], timeout=60)
        self.stable_state(lambda d: d["pdf_output_state"] == "Failed" and not d["pdf_output_pending"] and destination.read_bytes() == second_pdf and destination.stat().st_mtime_ns == second_mtime, frames=30)

        recovered = PREVIEW_SOURCE.replace("Black text on white paper.", "Recovered after a deliberate syntax error.")
        self.click("editor.source")
        self.key(0)
        self.paste_source(recovered)
        self.wait_target("build.pdf")
        self.click("build.pdf")
        self.wait("PDF action recovers after fixing the source", lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        recovered_pdf = destination.read_bytes()
        if not recovered_pdf.startswith(b"%PDF-") or recovered_pdf == second_pdf:
            raise AssertionError("PDF retry did not write corrected source")
        recovered_mtime = destination.stat().st_mtime_ns
        self.stable_state(lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and destination.read_bytes() == recovered_pdf and destination.stat().st_mtime_ns == recovered_mtime, frames=30)
        self.choose_backend("Pdfium", save_pdf=False)
        self.wait("PDFium cache build settles without exporting", lambda d: d["preview_backend"] == "PDFium" and d["preview_pdfium_ready"] and not d["compile_indicator_spinning"], timeout=60)
        self.stable_state(lambda d: not d["pdf_output_pending"] and destination.read_bytes() == recovered_pdf and destination.stat().st_mtime_ns == recovered_mtime)
        self.record("pdf.preview-only", bytes=len(recovered_pdf), unchanged_on_backend_switch=True)

        # Only an explicit CLI build writes the public PDF. A stable existing
        # output must not satisfy the check for the newly edited source.
        cli_source = recovered.replace("Recovered after a deliberate syntax error.", "Saved through the Typst compiler gear.")
        self.click("editor.source")
        self.key(0)
        self.paste_source(cli_source)
        self.wait("CLI edit refreshes only its private preview", lambda d: d["pdf_output_state"] == "Stale" and d["preview_pdfium_ready"] and not d["compile_indicator_spinning"] and not d["pdf_output_pending"], timeout=60)
        self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(cli_source) and destination.read_bytes() == recovered_pdf and destination.stat().st_mtime_ns == recovered_mtime)
        self.wait_target("build.preview")
        self.click("build.preview")
        self.wait("Typst gear saves the current PDF", lambda d: d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        cli_pdf = destination.read_bytes()
        if not cli_pdf.startswith(b"%PDF-") or cli_pdf == recovered_pdf:
            raise AssertionError("Typst gear did not save the edited PDF")
        cli_mtime = destination.stat().st_mtime_ns
        self.key(15, "cmd+shift")
        self.wait("CLI automatic builds pause", lambda d: d["compilation_paused"] and not d["compile_indicator_spinning"])
        paused_source = cli_source.replace("Saved through the Typst compiler gear.", "Saved once while automatic preview is paused.")
        self.click("editor.source")
        self.key(0)
        self.paste_source(paused_source)
        self.wait("paused edits retain the saved PDF", lambda d: d["compilation_paused"] and d["pdf_output_state"] == "Stale" and not d["pdf_output_pending"] and not d["compile_indicator_spinning"])
        self.stable_state(lambda d: d["compilation_paused"] and d["source_fingerprint"] == fingerprint(paused_source) and destination.read_bytes() == cli_pdf and destination.stat().st_mtime_ns == cli_mtime)
        self.click("build.preview")
        self.wait("paused gear creates a fresh PDF without resuming", lambda d: d["compilation_paused"] and d["pdf_output_state"] == "Saved" and not d["pdf_output_pending"] and not d["compile_indicator_spinning"] and str(d["pdf_output_revision"]) == d["revision"], timeout=60)
        paused_pdf = destination.read_bytes()
        if not paused_pdf.startswith(b"%PDF-") or paused_pdf == cli_pdf:
            raise AssertionError("paused compiler gear reused the previous source's PDF")
        paused_mtime = destination.stat().st_mtime_ns
        still_paused_source = paused_source.replace("Saved once while automatic preview is paused.", "This later edit must remain paused.")
        self.click("editor.source")
        self.key(0)
        self.paste_source(still_paused_source)
        self.wait("later edit remains paused after manual build", lambda d: d["compilation_paused"] and d["pdf_output_state"] == "Stale" and not d["pdf_output_pending"])
        self.stable_state(lambda d: d["compilation_paused"] and not d["compile_indicator_spinning"] and d["source_fingerprint"] == fingerprint(still_paused_source) and destination.read_bytes() == paused_pdf and destination.stat().st_mtime_ns == paused_mtime, frames=30)
        self.key(15, "cmd+shift")
        self.wait("automatic preview resumes explicitly", lambda d: not d["compilation_paused"])
        self.record("pdf.cli-gear", fresh_while_paused=True, remains_paused_after_build=True)

        # Restore aggregate-run ownership and settings without overwriting its files.
        self.tex_settings([('"auto_save": false', '"auto_save": true')])
        self.choose_backend(before["preview_requested_backend"], save_pdf=False)
        self.close_scratch(before["tabs"])
        if before["preview_tab_id"] is not None:
            self.click("tab." + str(before["preview_tab_id"]))
            self.key(35, "cmd+alt")
        if before["active_tab_id"] is not None:
            self.click("tab." + str(before["active_tab_id"]))
        if before["view_mode"] != "Split":
            self.key(19 if before["view_mode"] == "Code" else 21)

    def controls(self):
        for backend in ("Pdfium", "Interactive"):
            self.choose_backend(backend)
            self.wait("three-page preview loaded", lambda d: d["preview_pages"] == 3, timeout=30)
            if not self.snapshot()["document"]["preview_controls_open"]:
                if backend == "Pdfium":
                    target = self.snapshot()["targets"]["preview.open"]
                    self.native("move", target["x"], target["y"])
                    self.wait("toolbar tooltip appears before opening controls", lambda d: d["hover_tooltip_open"] and d["hover_tooltip_rect"] is not None)
                    card = self.snapshot()["document"]["hover_tooltip_rect"]
                    if card[2] >= 250:
                        raise AssertionError("short toolbar tooltip retained a documentation-sized minimum width")
                    if self.capture_review:
                        state = self.snapshot()["document"]
                        pointer = state["pointer"]
                        self.native("click", card[0] + card[2] / 2 + target["x"] - pointer[0],
                                    card[1] + card[3] / 2 + target["y"] - pointer[1])
                        self.capture_viewport("diagnostic")
                self.click("preview.open")
                self.wait("opening controls dismisses the toolbar tooltip", lambda d: not d["hover_tooltip_open"])
            self.wait("controls opened", lambda d: d["preview_controls_open"])
            before_position = self.wait("controls position measured", lambda d: d["preview_controls_position"] is not None)["document"]["preview_controls_position"]
            self.wait_target("preview.drag_handle")
            handle = self.snapshot()["targets"]["preview.drag_handle"]
            self.native("drag", handle["x"], handle["y"], handle["x"] + 48, handle["y"] + 24)
            self.wait("preview controls drag persists", lambda d: d["preview_controls_position"] is not None and
                      abs(d["preview_controls_position"][0] - before_position[0] - 48) <= 3 and
                      abs(d["preview_controls_position"][1] - before_position[1] - 24) <= 3)
            self.wait_target("preview.Outline")
            if not self.snapshot()["document"]["preview_outline"]:
                self.click("preview.Outline")
            self.wait("outline is open", lambda d: d["preview_outline"])
            size = self.wait("native controls measured", lambda d: d["preview_controls_size"] is not None and d["preview_controls_size"] == d["preview_controls_measured_size"])["document"]["preview_controls_size"]
            self.stable_state(lambda d: d["preview_controls_size"] == size, frames=30)
            if self.capture_review and backend == "Pdfium":
                self.capture_viewport("preview-controls")
            self.native("finder")
            self.wait("controls hide outside the app", lambda d: d["preview_controls_visible"] is False and d["preview_controls_open"])
            self.native("activate")
            self.wait("controls return with the app", lambda d: d["preview_controls_visible"] is True)
            self.wait_target("preview.outline.0")
            self.click("preview.outline.0")
            self.wait("start navigation at first page", lambda d: d["preview_page"] == 0)
            self.wait_target("preview.outline.1")
            self.click("preview.outline.1")
            self.wait("outline navigates to second page", lambda d: d["preview_page"] == 1 and d["preview_back"])
            self.click("preview.Back")
            self.wait("back restores first page", lambda d: d["preview_page"] == 0 and d["preview_forward"])
            self.click("preview.Forward")
            self.wait("forward restores second page", lambda d: d["preview_page"] == 1)
            self.click("preview.Next page")
            self.wait("next page", lambda d: d["preview_page"] == 2)
            self.stable_state(lambda d: d["preview_page"] == 2)
            self.click("preview.Previous page")
            self.wait("previous page", lambda d: d["preview_page"] == 1)
            before = self.snapshot()["document"]["preview_zoom"]
            self.click("preview.Zoom in")
            self.wait("zoom in", lambda d: d["preview_zoom"] > before)
            self.click("preview.Fit page width")
            self.wait("fit restores zoom", lambda d: abs(d["preview_zoom"] - before) < 0.01)
            self.click("preview.query")
            self.key(0)
            self.native("text", "Final needle")
            self.key(36, "none")
            self.wait("preview text search", lambda d: d["preview_page"] == 2)
            self.click("preview.Minimize controls")
            self.wait("controls minimized", lambda d: not d["preview_controls_open"])
            self.click("preview.open")
            self.wait("controls reopened", lambda d: d["preview_controls_open"])
            self.click("preview.Pop out")
            self.wait("preview detached", lambda d: d["preview_popout"])
            self.native_wait("native preview window exists", lambda ws: any(w["AXTitle"] == "tiptoptyp Preview" for w in ws))
            self.native("close", "tiptoptyp Preview")
            self.wait("preview returns to document", lambda d: not d["preview_popout"] and d["view_mode"] == "Split")

    def help_target(self, name, scroll_down=False):
        import time
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            state = self.snapshot()
            if help_target_visible(state, name):
                return
            scroll = state["scrolls"].get("settings.help")
            if scroll:
                self.native("move", scroll["x"], scroll["y"])
                self.native("wheel", scroll["x"], scroll["y"], -4 if scroll_down else 100)
            time.sleep(0.05)
        raise AssertionError("Help target was not fully visible after native scrolling: " + name)

    def help_tool_options(self, source_fingerprint):
        self.help_target("settings.help.tools")
        self.click("settings.help.tools")
        for tool in ("typst", "tinymist", "tectonic", "texlab", "badness", "tex-fmt"):
            header = "settings.help.tool." + tool
            self.help_target(header)
            self.click(header)
            self.help_target("settings.help.option." + tool + ".0", scroll_down=True)
            self.help_target("settings.help.option." + tool + ".9", scroll_down=True)
            self.wait(tool + " reference stays in Settings without editing source",
                      lambda d: d["settings_help_visible"] and d["settings_visible"]
                      and d["source_fingerprint"] == source_fingerprint)
            self.help_target(header)
            self.click(header)
        self.help_target("settings.help.tools")
        self.click("settings.help.tools")

    def editor_preferences(self):
        count = self.scratch("#let sample = {\n  // TODO: inspect\n  (1, 2, 3)\n}\n#sample\n")
        original = self.snapshot()["document"]["source_fingerprint"]
        self.settings()
        self.click("settings.help")
        self.wait("Help opens within Settings", lambda d: d["settings_help_visible"] and d["settings_visible"])
        self.help_tool_options(original)
        if self.capture_review:
            self.capture_viewport("settings")
        self.key(3)
        self.wait_target("settings.search")
        self.wait("Find returns to Settings search", lambda d: not d["settings_help_visible"] and d["settings_text_input_focused"])
        for query, title, target, field in [
            ("guides", "Indentation guides", "settings.editor.guides", "indentation_guides"),
            ("local definitions", "Highlight uses of local definitions", "settings.editor.local-uses", "highlight_local_uses"),
            ("wrap", "Wrap lines", "settings.editor.wrap", "line_wrap"),
        ]:
            self.setting_section(query, title)
            before = self.snapshot()["document"][field]
            self.wait_target(target)
            self.click(target)
            self.wait("preference applies: " + field, lambda d: d[field] != before)
            self.click(target)
            self.wait("preference restores: " + field, lambda d: d[field] == before)
        self.setting_section("highlights", "Custom text highlights")
        self.wait_target("settings.highlight.1")
        self.click("settings.highlight.1")
        self.key(0)
        self.native("text", "FIXME:")
        self.wait("rule typing belongs to Settings", lambda d: d["settings_text_input_focused"] and d["source_fingerprint"] == original)
        if self.capture_review:
            self.capture_viewport("settings")
        self.close_settings()
        self.wait("preferences do not edit source", lambda d: d["source_fingerprint"] == original)
        self.typing_particles()
        self.wait("particle scratch leaves the preference fixture intact", lambda d: d["source_fingerprint"] == original)
        if self.capture_review:
            self.capture_viewport("main")
        self.close_scratch(count)

    def typing_particle_burst(self, text, expected, capture=False):
        self.editor_particle_action("Typing", expected, lambda: self.native("text", text), capture=capture)

    def editor_particle_action(self, editor_action, expected, perform=None,
                               copy_text=None, paste_text=None, capture=False):
        before = self.snapshot()["document"]
        def observe():
            active = self.wait("real editor action renders one native GPU burst: " + editor_action,
                               lambda d: particle_action_rendered(d, before, editor_action, expected))
            self.record("typing_particles.active", editor_action=editor_action,
                        style=active["document"]["editor_effects"]["actions"][editor_action.lower()]["particles"],
                        dark=active["document"]["dark"],
                        observation=active["document"]["typing_particles"])
            if capture:
                self.capture_viewport("main")
        if paste_text is not None:
            self.paste_source(paste_text, check=observe)
        elif copy_text is not None:
            self.copy_with_receipt(copy_text, perform, after_copy=observe)
        else:
            perform()
            observe()
        self.source_is(expected)
        expired = self.wait("editor particles expire without further input", lambda d:
                            typing_particle_state(d)["active_count"] == 0, timeout=3)["document"]
        bursts, draws = typing_particle_state(expired)["burst_count"], typing_particle_gpu_state(expired)["draws"]
        self.stable_state(lambda d: typing_particle_state(d)["active_count"] == 0
                          and typing_particle_state(d)["burst_count"] == bursts
                          and typing_particle_gpu_state(d)["draws"] == draws, frames=3)

    def editor_particle_commands(self, expected):
        for action, style in (("Comment", "Shotgun"), ("Copy", "Railgun"), ("Paste", "Plasma"),
                              ("Cut", "Flamethrower"), ("Undo", "Explosion"), ("Redo", "Shotgun"),
                              ("Newline", "Railgun"), ("Delete", "Plasma"), ("Indent", "Flamethrower")):
            self.editor_effect_choose(action, particles=style)
        def eof():
            self.click("editor.source")
            self.key(125)
            self.wait("editor action caret is at EOF", lambda d:
                      d["editor_keyboard_focused"] and d["cursor"] == [len(expected), len(expected)])

        def select_tail(length):
            eof()
            whole_line = length == len(expected.rsplit("\n", 1)[-1])
            if whole_line:
                self.key(123, "cmd+shift")
                steps = (length,)
            else:
                steps = range(1, length + 1)
            for selected in steps:
                if not whole_line:
                    self.key(123, "shift")
                self.wait("editor action confirms each native selection step", lambda d:
                          d["editor_keyboard_focused"]
                          and d["source_fingerprint"] == fingerprint(expected)
                          and sorted(d["cursor"]) == [len(expected) - selected, len(expected)])

        def popup():
            state = self.snapshot()
            before = typing_particle_state(state["document"])["burst_count"]
            selection = state["document"]["cursor"]
            self.right_click(editor_particle_popup_target(state))
            self.wait_target("menu.Paste")
            self.stable_state(lambda d: d["source_fingerprint"] == fingerprint(expected)
                              and d["cursor"] == selection
                              and typing_particle_state(d)["burst_count"] == before, frames=3)

        eof()
        prefix, line = expected.rsplit("\n", 1)
        commented = prefix + "\n// " + line
        self.editor_particle_action("Comment", commented, lambda: self.key(44), capture=self.capture_review)
        self.editor_particle_action("Comment", expected, lambda: self.key(44), capture=self.capture_review)
        select_tail(len(line))
        self.editor_particle_action("Copy", expected, lambda: self.key(8), copy_text=line, capture=self.capture_review)
        popup()
        self.editor_particle_action("Copy", expected, lambda: self.click("menu.Copy"), copy_text=line, capture=self.capture_review)
        popup()
        self.editor_particle_action("Comment", commented, lambda: self.click("menu.Toggle Comment"), capture=self.capture_review)
        self.editor_particle_action("Comment", expected, lambda: self.key(44), capture=self.capture_review)
        eof()
        suffix = " pasted"
        pasted = expected + suffix
        self.editor_particle_action("Paste", pasted, paste_text=suffix, capture=self.capture_review)
        expected = pasted
        select_tail(len(suffix))
        expected = pasted[:-len(suffix)]
        self.editor_particle_action("Cut", expected, lambda: self.key(7), copy_text=suffix, capture=self.capture_review)
        self.editor_particle_action("Undo", pasted, lambda: self.key(6), capture=self.capture_review)
        self.editor_particle_action("Redo", expected, lambda: self.key(6, "cmd+shift"), capture=self.capture_review)
        eof()
        self.editor_particle_action("Indent", expected + "  ", lambda: self.key(48, "none"), capture=self.capture_review)
        self.editor_particle_action("Undo", expected, lambda: self.key(6), capture=self.capture_review)
        eof()
        self.editor_particle_action("Newline", expected + "\n", lambda: self.key(36, "none"), capture=self.capture_review)
        self.editor_particle_action("Delete", expected, lambda: self.key(51, "none"), capture=self.capture_review)
        # Opening/dismissing a command popup does not execute an editor action.
        popup()
        self.key(53, "none")
        self.source_is(expected)
        return expected

    def editor_effect_choose(self, action, particles=None, shake=False, flash=False, glow=False):
        before_document = self.snapshot()["document"]
        before = typing_particle_state(before_document)
        history = {field: before[field] for field in (
            "burst_count", "emitted_count", "last_style", "last_action", "action_bursts")}
        feedback = editor_feedback_state(before_document)
        feedback_history = {field: deepcopy(feedback[field]) for field in (
            "burst_count", "action_bursts", "last_action", "last_effects")}
        preferences = deepcopy(editor_effect_preferences(before_document))
        effects = {"particles": particles, "shake": shake, "flash": flash, "glow": glow}
        self.settings()
        self.setting_section("editor effects", "Editor effects")
        prefix = "settings.editor.effects." + action.lower()
        current = preferences["actions"][action.lower()]
        if current["particles"] != particles:
            self.click(prefix + ".particles")
            self.wait_target(prefix + ".particles." + (particles or "Off"))
            self.click(prefix + ".particles." + (particles or "Off"))
        for effect in ("shake", "flash", "glow"):
            if current[effect] != effects[effect]:
                self.click(prefix + "." + effect)
        preferences["actions"][action.lower()] = effects
        self.wait("only the selected action's effects change: " + action, lambda d:
                  editor_effect_preferences(d) == preferences)
        self.stable_state(lambda d: all(typing_particle_state(d)[field] == value
                                       for field, value in history.items())
                          and all(editor_feedback_state(d)[field] == value
                                  for field, value in feedback_history.items()), frames=3)
        self.close_settings()
        self.record("typing_particles.action_preferences", editor_action=action, effects=effects,
                    preserved_history=history)

    def editor_feedback_actions(self, expected):
        from concurrent.futures import ThreadPoolExecutor
        for action, effect, text, perform in (
                ("Typing", "Shake", "s", lambda: self.native("text", "s")),
                ("Indent", "Flash", "  ", lambda: self.key(48, "none")),
                ("Newline", "Glow", "\n", lambda: self.key(36, "none"))):
            self.editor_effect_choose(action, **{effect.lower(): True})
            self.click("editor.source")
            self.key(125)
            self.wait("nonparticle action caret is at EOF", lambda d:
                      d["editor_keyboard_focused"] and d["cursor"] == [len(expected), len(expected)])
            before = self.snapshot()["document"]
            expected += text
            # Observe concurrently: native event posting includes a key-up delay,
            # while Flash intentionally has a much shorter lifetime than particles.
            with ThreadPoolExecutor(max_workers=1) as observer:
                active = observer.submit(self.wait, "particle-Off action renders " + effect,
                                         lambda d: editor_feedback_action_rendered(
                                             d, before, action, effect, expected))
                perform()
                active = active.result()
            self.record("typing_particles.feedback", editor_action=action, effect=effect,
                        observation=active["document"]["typing_particles"])
            if self.capture_review:
                self.capture_viewport("main")
            self.source_is(expected)
            expired = self.wait("nonparticle feedback expires without input", lambda d:
                                not editor_feedback_state(d)["active_effects"], timeout=3)["document"]
            burst_count = editor_feedback_state(expired)["burst_count"]
            draws = typing_particle_gpu_state(expired)["draws"]
            self.stable_state(lambda d: not editor_feedback_state(d)["active_effects"]
                              and editor_feedback_state(d)["burst_count"] == burst_count
                              and typing_particle_gpu_state(d)["draws"] == draws, frames=3)
        # All individual effects Off must be a true no-op, including no GPU work.
        self.editor_effect_choose("Typing")
        self.click("editor.source")
        self.key(125)
        before = self.snapshot()["document"]
        expected += "o"
        self.native("text", "o")
        self.source_is(expected)
        self.stable_state(lambda d: editor_feedback_state(d)["burst_count"]
                          == editor_feedback_state(before)["burst_count"]
                          and not editor_feedback_state(d)["active_effects"]
                          and typing_particle_state(d)["burst_count"] == typing_particle_state(before)["burst_count"]
                          and typing_particle_gpu_state(d)["draws"] == typing_particle_gpu_state(before)["draws"], frames=3)
        return expected

    def typing_particles(self):
        launch = self.snapshot()["document"]
        needs_source_preview = (launch["tabs"] == 1
                                and (launch["path"] or "").endswith("/journey.txt"))
        initial = "\n" * 8 + "particle "
        count = self.scratch(initial)
        if needs_source_preview:
            self.wait("first unsaved source adopts preview ownership", lambda d:
                      d["path"] is None and d["active_tab_id"] == d["preview_tab_id"])
            self.stable_state(lambda d:
                              "Could not read preview entry" not in (d["completion_notice"] or "")
                              and "Could not read preview entry" not in (d["preview_failure"] or ""),
                              frames=3)
        document = self.snapshot()["document"]
        appearance = document["interface_theme"]
        initial_dark = document["dark"]
        preferences = deepcopy(editor_effect_preferences(document))
        state = typing_particle_state(document)
        typing_particle_gpu_state(document)
        if preferences != {"enabled": False, "density": "Medium", "actions": {
                action.lower(): {"particles": "Explosion", "shake": False, "flash": False, "glow": False}
                for action in PARTICLE_ACTIONS}} or state["active_count"] or state["burst_count"]:
            raise AssertionError("editor effects must default off at medium density with Explosion for every action")
        expected = initial
        try:
            self.settings()
            self.setting_section("editor effects", "Editor effects")
            self.click("settings.editor.effects.enabled")
            self.wait("particle checkbox enables the real setting", lambda d: d["editor_effects"]["enabled"])
            if self.capture_review:
                self.capture_viewport("settings")
            self.close_settings()
            for style, text in zip(PARTICLE_STYLES, "esfrp"):
                self.editor_effect_choose("Typing", particles=style)
                self.click("editor.source")
                self.key(125)  # Cmd+Down reaches EOF even after clicking a blank line.
                self.wait("source owns particle input at EOF", lambda d:
                          d["editor_keyboard_focused"] and d["cursor"] == [len(expected), len(expected)])
                expected += text
                self.typing_particle_burst(text, expected, capture=self.capture_review)
            for theme, dark, text in (("Light", False, "l"), ("Dark", True, "d")):
                self.settings()
                self.setting_section("appearance", "Appearance")
                self.click("settings.theme." + theme)
                self.wait("GPU particle appearance applies: " + theme, lambda d:
                          d["interface_theme"] == theme and d["dark"] is dark)
                self.close_settings()
                self.click("editor.source")
                self.key(125)
                expected += text
                self.typing_particle_burst(text, expected, capture=self.capture_review)
            expected = self.editor_particle_commands(expected)
            expected = self.editor_feedback_actions(expected)
            # Keep every feedback kind configured for disabled/foreign-focus negatives.
            self.editor_effect_choose("Typing", particles="Plasma", shake=True, flash=True, glow=True)
            quiet_document = self.snapshot()["document"]
            before = typing_particle_state(quiet_document)["burst_count"]
            idle_draws = typing_particle_gpu_state(quiet_document)["draws"]
            feedback_bursts = editor_feedback_state(quiet_document)["burst_count"]
            def remains_quiet(document):
                state = typing_particle_state(document)
                gpu = typing_particle_gpu_state(document)
                feedback = editor_feedback_state(document)
                return (state["burst_count"] == before and state["active_count"] == 0
                        and feedback["burst_count"] == feedback_bursts and not feedback["active_effects"]
                        and gpu["draws"] == idle_draws)
            self.key(123, "none")  # Moving a caret must not emit another burst.
            self.stable_state(remains_quiet, frames=3)
            self.settings()
            self.click("settings.search")
            self.key(0)
            self.native("text", "particle focus check")
            self.wait("Settings typing stays outside the source editor", lambda d:
                      d["settings_text_input_focused"]
                      and d["settings_query_fingerprint"] == fingerprint("particle focus check")
                      and d["source_fingerprint"] == fingerprint(expected))
            self.stable_state(remains_quiet, frames=3)
            self.key(6)
            self.key(6, "cmd+shift")
            self.stable_state(lambda d: remains_quiet(d)
                              and d["source_fingerprint"] == fingerprint(expected), frames=3)
            self.close_settings()
            # Empty history commands are no-ops even while the feature is on.
            empty_count = self.scratch()
            empty = self.snapshot()["document"]
            empty_bursts = typing_particle_state(empty)["burst_count"]
            empty_draws = typing_particle_gpu_state(empty)["draws"]
            empty_feedback = editor_feedback_state(empty)["burst_count"]
            self.key(6)
            self.key(6, "cmd+shift")
            self.stable_state(lambda d: d["source_fingerprint"] == fingerprint("")
                              and typing_particle_state(d)["burst_count"] == empty_bursts
                              and typing_particle_state(d)["active_count"] == 0
                              and editor_feedback_state(d)["burst_count"] == empty_feedback
                              and not editor_feedback_state(d)["active_effects"]
                              and typing_particle_gpu_state(d)["draws"] == empty_draws, frames=3)
            self.close_scratch(empty_count)
            self.source_is(expected)
            # Switching documents resets the scoped trace. Prove no hidden replay
            # before accepting zero as the baseline for subsequent disabled checks.
            self.stable_state(lambda d: particle_document_returned_quiet(d, quiet_document, expected), frames=3)
            self.record("typing_particles.document_return", previous_burst_count=before,
                        reset_burst_count=0, idle_gpu_draws=idle_draws)
            before = 0
            feedback_bursts = 0
            self.settings()
            self.setting_section("editor effects", "Editor effects")
            self.click("settings.editor.effects.enabled")
            self.wait("particle checkbox disables the real setting", lambda d: not d["editor_effects"]["enabled"])
            self.close_settings()
            self.click("editor.source")
            self.key(125)
            self.native("text", "z")
            self.source_is(expected + "z")
            self.stable_state(remains_quiet, frames=3)
            self.record("typing_particles.negative", burst_count=before, idle_gpu_draws=idle_draws)
        finally:
            self.settings()
            self.setting_section("editor effects", "Editor effects")
            current = editor_effect_preferences(self.snapshot()["document"])
            for action, effects in preferences["actions"].items():
                if current["actions"][action] != effects:
                    self.close_settings()
                    self.editor_effect_choose(action, **effects)
                    self.settings()
                    self.setting_section("editor effects", "Editor effects")
            if current["enabled"] != preferences["enabled"]:
                self.click("settings.editor.effects.enabled")
            self.wait("effect journey restores every initial per-action preference", lambda d:
                      editor_effect_preferences(d) == preferences)
            self.setting_section("appearance", "Appearance")
            self.click("settings.theme." + ("Follow system" if appearance == "System" else appearance))
            self.wait("particle journey restores the exact initial appearance", lambda d:
                      d["interface_theme"] == appearance and d["dark"] is initial_dark)
            self.close_settings()
            self.click("editor.source")
            self.record("typing_particles.restored", preferences=preferences, appearance=appearance)
        self.close_scratch(count)

    def settings_json_select_all(self):
        """Wait for the native Select All command before replacing the JSON draft."""
        import time
        self.wait_target("settings.json", focused=True)
        state = self.snapshot()
        draft = state["document"]["settings_json"]["fingerprint"]
        if not isinstance(draft, str):
            raise AssertionError("Settings JSON draft fingerprint is unavailable")
        selection = settings_json_editor_selection(state, draft)
        # An already selected draft needs no duplicate asynchronous menu command.
        if selection is not None and min(selection) == 0 and max(selection) > 0:
            return draft, max(selection)
        self.key(0)
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            selection = settings_json_editor_selection(self.snapshot(), draft)
            if selection is not None and min(selection) == 0 and max(selection) > 0:
                return draft, max(selection)
            time.sleep(0.05)
        raise AssertionError("Settings JSON did not acknowledge Select All")

    def settings_json_append_whitespace(self):
        """Prove selection collapse before typing, then observe the changed valid draft."""
        import time
        draft, end = self.settings_json_select_all()
        self.key(124, "none")
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            if settings_json_editor_selection(self.snapshot(), draft) == [end, end]:
                break
            time.sleep(0.05)
        else:
            raise AssertionError("Settings JSON Right did not collapse the acknowledged selection at its end")
        self.native("text", " ")
        self.wait("changed JSON remains valid after appending whitespace", lambda d:
                  d["settings_json"]["fingerprint"] != draft and d["settings_json"]["valid"])

    def settings_category(self, category, query, source, size):
        import time
        if not settings_category_target_visible(self.snapshot(), category):
            self.click("settings.category.selector")
        deadline = time.monotonic() + 6
        while not settings_category_target_visible(self.snapshot(), category):
            if time.monotonic() >= deadline:
                raise AssertionError("Settings category target is not current: " + category)
            time.sleep(0.05)
        self.click("settings.category." + category)
        self.wait("category retains search and source: " + category, lambda d:
                  settings_navigation_matches(d, category, query, source))
        if settings_window_size(self.native("windows")["windows"]) != size:
            raise AssertionError("switching Settings category resized the native window")

    def settings_search(self):
        self.settings()
        if self.snapshot()["document"]["settings_json"]["visible"]:
            self.click("settings.json.mode")
            self.wait("category journey starts in the Settings form", lambda d:
                      not d["settings_json"]["visible"])
        before = self.snapshot()["document"]
        tabs, panel, source = before["tabs"], before["panel"], before["source_fingerprint"]
        size = settings_window_size(self.native("windows")["windows"])
        self.key(45)  # Cmd+N belongs to Settings; it must not create a document.
        self.stable_state(lambda d: d["settings_visible"] and d["tabs"] == tabs)
        self.key(23)  # Cmd+5 must not toggle the document's bottom panel.
        self.stable_state(lambda d: d["settings_visible"] and d["panel"] == panel)
        self.key(3)  # Cmd+F focuses the Settings search, not document Find.
        self.wait("Settings search receives its shortcut", lambda d: d["settings_text_input_focused"] and not d["find_visible"])
        self.key(0)
        self.native("text", "backend")
        self.wait("Settings search receives typed text", lambda d: d["settings_query_fingerprint"] == fingerprint("backend"))
        self.key(3)
        self.wait("repeated Cmd+F keeps Settings search focused", lambda d: d["settings_text_input_focused"] and not d["find_visible"])
        for category in ("Appearance", "Editor", "Writing", "Grammar & spelling", "Tools"):
            self.settings_category(category, "backend", source, size)
        self.setting_section("wrap", "Wrap lines")
        self.wait("search routes to Editor", lambda d:
                  settings_navigation_matches(d, "Editor", "wrap", source))
        wrap = self.snapshot()["document"]["line_wrap"]
        self.click("settings.editor.wrap")
        self.wait("form edit applies before category navigation", lambda d: d["line_wrap"] != wrap)
        for category in ("Writing", "Editor"):
            self.settings_category(category, "wrap", source, size)
            self.stable_state(lambda d: d["line_wrap"] != wrap, frames=3)
        self.click("settings.editor.wrap")
        self.wait("form edit restores after category navigation", lambda d: d["line_wrap"] == wrap)
        for query, label, category in (
            ("backend", "Typst preview backend", "Preview"),
            ("Harper", "Harper spelling and grammar", "Grammar & spelling"),
            ("LTeX", "LTeX+ spelling and grammar", "Grammar & spelling"),
            ("Liquid", "Liquid selection grammar", "Grammar & spelling"),
            ("compiler", "Typst compiler", "Tools"),
            ("appearance", "Appearance", "Appearance"),
        ):
            self.setting_section(query, label)
            self.wait("search destination highlights the correct category", lambda d:
                      d["settings_highlight"] == label
                      and settings_navigation_matches(d, category, query, source))
            if settings_window_size(self.native("windows")["windows"]) != size:
                raise AssertionError("search navigation resized the native Settings window")
            if self.capture_review and label == "Appearance":
                self.capture_viewport("settings")
        self.wait("highlight expires", lambda d: d["settings_highlight"] is None, timeout=5)
        self.setting_section("dark images", "Apply dark page appearance to images")
        images = self.snapshot()["document"]["dark_images"]
        self.click("settings.page.Dark")
        self.wait("sample uses dark page appearance", lambda d: d["page_dark"])
        if self.capture_review:
            self.capture_viewport("settings")
        self.click("settings.dark_images")
        self.wait("image appearance preference applies independently", lambda d:
                  d["dark_images"] != images and d["source_fingerprint"] == source)
        if self.capture_review:
            self.capture_viewport("settings")
        self.click("settings.dark_images")
        self.wait("image appearance preference restored", lambda d: d["dark_images"] == images)
        self.click("settings.page.FollowInterface")
        self.setting_section("appearance", "Appearance")
        self.click("settings.json.mode")
        self.wait("JSON settings opens with valid current settings", lambda d: d["settings_json"]["visible"] and d["settings_json"]["valid"])
        if self.capture_review:
            self.capture_viewport("settings")
        self.click("settings.json")
        self.wait_target("settings.json", focused=True)
        self.settings_json_select_all()
        self.native("text", "{broken")
        self.wait("invalid JSON replaces the complete draft and cannot be saved", lambda d:
                  d["settings_json"]["fingerprint"] == fingerprint("{broken")
                  and not d["settings_json"]["valid"])
        state = self.snapshot()
        if state["targets"]["settings.json.save"]["enabled"]:
            raise AssertionError("invalid settings left Save enabled")
        draft = state["document"]["settings_json"]["fingerprint"]
        self.click("settings.json.mode")
        self.wait("form navigation leaves the JSON draft parked", lambda d:
                  not d["settings_json"]["visible"])
        for category in ("Editor", "Tools"):
            self.settings_category(category, "appearance", source, size)
        self.click("settings.json.mode")
        self.wait("JSON draft survives form category navigation", lambda d:
                  d["settings_json"]["visible"] and not d["settings_json"]["valid"]
                  and d["settings_json"]["fingerprint"] == draft)
        self.click("settings.json.reload")
        self.wait("Reload restores valid settings", lambda d: d["settings_json"]["valid"])
        self.click("settings.json")
        self.wait_target("settings.json", focused=True)
        self.settings_json_append_whitespace()
        self.click("settings.json.save")
        self.wait("JSON saves through normal settings updates", lambda d: d["settings_json"]["saved"])
        settings_window = next(w for w in self.native("windows")["windows"] if "Settings" in w["AXTitle"])
        size = settings_window["size"]
        self.native("zoom-focused")
        self.native_wait("Settings maximizes independently", lambda ws: any(w.get("AXTitle") == settings_window["AXTitle"] and w.get("size") != size for w in ws))
        self.native("zoom-focused")
        self.native_wait("Settings restores its previous size", lambda ws: any(w.get("AXTitle") == settings_window["AXTitle"] and w.get("size") == size for w in ws))
        self.key(3)
        self.wait("Cmd+F stays in JSON", lambda d: d["settings_json"]["visible"] and not d["find_visible"])
        self.wait_target("settings.json.find", focused=True)
        self.key(0)
        self.native("text", "writing_language")
        self.wait("JSON find receives typing", lambda d: d["settings_json"]["query"] == "writing_language")
        self.key(13)  # Cmd+W closes Settings, not the document tab.
        self.wait("Cmd+W closes Settings but preserves the document", lambda d:
                  not d["settings_visible"] and d["tabs"] == tabs
                  and d["source_fingerprint"] == source)
        self.native_wait("document regains focus after Settings shortcut", lambda ws: not any("Settings" in w["AXTitle"] for w in ws) and any(w.get("AXFocused") for w in ws))

    def background(self):
        self.ensure_typesetting_tab()
        self.choose_backend("Pdfium")
        self.key(19)
        self.wait("preview pane hidden", lambda d: d["view_mode"] == "Code")
        self.choose_backend("Interactive")
        self.wait("hidden frontend created", lambda d: d["preview_surface_exists"] and not d["preview_surface_visible"])
        import time
        deadline = time.monotonic() + 30
        while True:
            state = self.snapshot()
            renderer = state.get("renderer")
            if renderer and renderer["age_ms"] < 500 and renderer["value"] and renderer["value"].get("loaded"):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("hidden Tinymist frontend did not finish loading")
            time.sleep(0.05)
        generation = state["renderer_generation"]
        self.key(20)
        self.wait("prepared preview revealed", lambda d: d["view_mode"] == "Split" and d["preview_surface_visible"])
        # WebKit may defer animation-frame painting for a hidden NSView; loading
        # the frontend must finish before reveal, without requiring hidden pixels.
        deadline = time.monotonic() + 10
        while True:
            state = self.snapshot()
            renderer = state.get("renderer")
            if renderer and renderer["age_ms"] < 500 and renderer["value"] and renderer["value"].get("ready"):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("prepared Tinymist viewer did not paint on reveal")
            time.sleep(0.05)
        for _ in range(8):
            if self.snapshot()["renderer_generation"] != generation:
                raise AssertionError("revealing the prepared preview reloaded it")

        # WindowServer acceptance is configuration evidence, not a readback or
        # pixel proof. Composed native appearance still needs visual inspection.
        def blur_applied(d, key="native_background"):
            background = d.get(key) or {}
            blur = background.get("blur") or {}
            return (blur.get("method") == "window-server"
                    and blur.get("applied_radius") == background.get("blur_radius"))
        def retained_blur(d, before, key="native_background"):
            return (blur_applied(d, key)
                    and d[key]["blur"]["identity"] == before["blur"]["identity"]
                    and d[key]["blur"]["applications"] == before["blur"]["applications"])
        self.wait("WindowServer accepted the requested document blur radius", blur_applied)
        original = self.snapshot()["document"]["source_fingerprint"]
        for backend in ("Interactive", "Pdfium"):
            self.choose_backend(backend)
            self.wait("effective preview backend is ready with blur", lambda d:
                      d["preview_native_ready"] if backend == "Interactive" else d["preview_pdfium_ready"], timeout=40)
            before_settings = self.snapshot()["document"]["native_background"]
            self.settings()
            self.setting_section("appearance", "Appearance")
            self.wait("Settings has its own native blur controller", lambda d:
                      blur_applied(d, "settings_native_background"))
            self.wait("Settings focus preserves the document blur controller and accepted radius", lambda d:
                      retained_blur(d, before_settings)
                      and blur_applied(d, "settings_native_background"))
            for _ in range(3):
                self.click("settings.background.blur")
                self.wait("blur off releases the document blur controller", lambda d: d["native_background"]["blur"] is None)
                self.wait("blur off releases the Settings blur controller", lambda d: d["settings_native_background"]["blur"] is None)
                self.click("settings.background.blur")
                self.wait("blur on restores both WindowServer blur configurations", lambda d:
                          blur_applied(d) and blur_applied(d, "settings_native_background"))
            before_theme = self.snapshot()["document"]
            self.click("settings.theme.Light")
            self.wait("light theme retains both native blur configurations without reapplying", lambda d:
                      not d["native_background"]["dark"] and not d["settings_native_background"]["dark"]
                      and retained_blur(d, before_theme["native_background"])
                      and retained_blur(d, before_theme["settings_native_background"], "settings_native_background"))
            self.click("settings.theme.Dark")
            self.wait("dark theme retains both native blur configurations without reapplying", lambda d:
                      d["native_background"]["dark"] and d["settings_native_background"]["dark"]
                      and retained_blur(d, before_theme["native_background"])
                      and retained_blur(d, before_theme["settings_native_background"], "settings_native_background"))
            if self.capture_review:
                self.capture_viewport("settings")
            self.close_settings()
            self.click("editor.source")
            self.key(3)
            self.wait("Find gets keyboard ownership with blur", lambda d: d["find_visible"] and d["find_focused"])
            self.wait("Find focus preserves the document blur controller and accepted radius", lambda d:
                      retained_blur(d, before_theme["native_background"]))
            self.key(0)  # Find retains its query across both backend checks.
            self.native("text", "Desktop")
            self.wait("Find receives native typing with blur", lambda d: d["find_query_fingerprint"] == fingerprint("Desktop"))
            self.key(53, "none")
            self.wait("Find closes once", lambda d: not d["find_visible"])
            before_state = self.snapshot()["document"]
            before = before_state["native_background"]
            size = before_state["native_size"]
            self.native("resize-focused", 1200, 720)
            self.wait("ordinary resize retains the blur controller without another API call", lambda d:
                      native_size_matches(d["native_size"], [1200.0, 720.0]) and retained_blur(d, before))
            self.native("resize-focused", *size)
            self.wait("ordinary resize restore retains the same blur configuration", lambda d:
                      native_size_matches(d["native_size"], size) and retained_blur(d, before))
            self.stable_state(lambda d: retained_blur(d, before))
            preferences = self.snapshot()["document"]["window_background"]
            # On macOS the green native AXZoomButton enters fullscreen. Test
            # that path and the app shortcut, rather than calling it a resize.
            self.click("editor.source")
            if backend == "Interactive":
                self.native("zoom-focused")
            else:
                self.key(3, "cmd+ctrl")
            self.wait("fullscreen disables transparency and removes blur", lambda d:
                      d["fullscreen"] and not d["native_background"]["translucent"]
                      and d["native_background"]["blur"] is None and d["window_background"] == preferences)
            self.stable_state(lambda d: d["fullscreen"] and not d["native_background"]["translucent"], frames=24)
            if self.capture_review:
                self.capture_viewport("main")
            if backend == "Interactive":
                self.native("zoom-focused")
            else:
                self.key(3, "cmd+ctrl")
            self.wait("leaving fullscreen restores saved transparency and blur", lambda d:
                      not d["fullscreen"] and d["native_background"]["translucent"]
                      and blur_applied(d) and d["window_background"] == preferences)
            self.stable_state(lambda d: not d["fullscreen"] and blur_applied(d), frames=24)
            if self.capture_review:
                self.capture_viewport("main")
        self.wait("blur transitions never edit the document", lambda d: d["source_fingerprint"] == original)
        if self.review_preview:
            self.click("editor.source")
            self.settings()
            self.setting_section("appearance", "Appearance")
            self.review_native_preview("background-blur", document_focus=False)
            self.close_settings()

    def background_fades(self):
        """Independent native blur and tint gradients survive focus/fullscreen."""
        self.focus_document_window()
        before = self.snapshot()["document"]
        source = before["source_fingerprint"]
        windows = self.native("windows")["windows"]
        document_title = next(window["AXTitle"] for window in windows if window.get("AXFocused"))
        self.settings()
        self.setting_section("appearance", "Appearance")
        settings_title = next(window["AXTitle"] for window in self.native("windows")["windows"]
                              if "Settings" in window["AXTitle"])
        preferences = self.snapshot()["document"]["window_background"]
        if not preferences["enabled"] or not preferences["blur"] or not 0 <= preferences["opacity_percent"] < 100 or preferences["blur_radius"] <= 0:
            raise AssertionError("directional background journey requires active translucent blur")

        def select(kind, direction):
            field = kind + "_fade"
            selected = direction.lower()
            current = self.snapshot()["document"]
            if current["window_background"][field] == selected:
                return
            unchanged = current["window_background"]["blur_fade" if kind == "transparency" else "transparency_fade"]
            target = "settings.background." + field
            self.wait_target(target)
            self.click(target)
            self.wait_target(target + "." + direction)
            self.click(target + "." + direction)
            self.wait("direction preference applies independently: " + field, lambda d:
                      d["window_background"][field] == selected
                      and d["window_background"]["blur_fade" if kind == "transparency" else "transparency_fade"] == unchanged)
            expected = self.snapshot()["document"]["window_background"]
            self.wait("both windows apply their actual directional background", lambda d:
                      background_fades_applied(d, expected["blur_fade"], expected["transparency_fade"]))
            if kind == "transparency":
                self.stable_state(lambda d: background_blur_unchanged(d, current))

        for blur, transparency in (("Right", "None"), ("Right", "Down"),
                                   ("Up", "Down"), ("Up", "Left"), ("Right", "Down")):
            select("blur", blur)
            select("transparency", transparency)
            self.wait("chosen independent fade pair applied", lambda d:
                      background_fades_applied(d, blur.lower(), transparency.lower()))
            self.record("background_fades.configuration", blur=blur.lower(), transparency=transparency.lower())
        for field, target in (("blur_end_radius", "settings.background.blur_end_radius"),
                              ("opacity_end_percent", "settings.background.opacity_end_percent")):
            prior = self.snapshot()["document"]
            preferences = prior["window_background"]
            self.wait_target(target)
            # The two native windows repaint independently. Wait for this
            # Settings control's hover/geometry, not just a fresh root snapshot,
            # before posting one click on the actual track (outside its label).
            import time
            deadline = time.monotonic() + 3
            while True:
                previous = self.snapshot()["targets"][target]
                x = previous["x"] - previous["width"] / 2 + 35
                y = previous["y"]
                self.native("move", x, y)
                observed = self.snapshot()["targets"][target]
                if (observed["frame"] > previous["frame"] and observed["hovered"]
                        and abs(observed["x"] - previous["x"]) <= 0.5
                        and abs(observed["y"] - previous["y"]) <= 0.5):
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError("endpoint track did not settle: " + field)
            self.native("click", x, y)
            self.wait("endpoint slider updates only its own preference: " + field, lambda d:
                      d["window_background"][field] != preferences[field]
                      and all(d["window_background"][other] == value for other, value in preferences.items() if other != field)
                      and background_fades_applied(d, "right", "down"))
            if field == "opacity_end_percent":
                self.stable_state(lambda d: background_blur_unchanged(d, prior))
            self.record("background_fades.endpoint", field=field,
                        value=self.snapshot()["document"]["window_background"][field])
        baseline = self.snapshot()["document"]
        for _ in range(3):
            self.native("raise", document_title)
            self.wait("document focus retains both directional controllers", lambda d:
                      d["focused"] and d["settings_visible"] and background_blur_unchanged(d, baseline))
            self.click("editor.source")
            self.key(3)
            self.wait("Find owns typing above the directional background", lambda d: d["find_visible"] and d["find_focused"])
            self.key(0)
            self.native("text", "First")
            self.wait("Find typing cannot edit source or reapply blur", lambda d:
                      d["find_query_fingerprint"] == fingerprint("First")
                      and d["source_fingerprint"] == source and background_blur_unchanged(d, baseline))
            self.key(53, "none")
            self.wait("Find closes without changing directional backgrounds", lambda d:
                      not d["find_visible"] and background_blur_unchanged(d, baseline))
            self.native("raise", settings_title)
            self.native_wait("Settings regains native focus", lambda windows:
                             any(window["AXTitle"] == settings_title and window.get("AXFocused") for window in windows))
            self.stable_state(lambda d: background_fades_applied(d, "right", "down")
                              and background_blur_unchanged(d, baseline), frames=12)
        if self.review_preview:
            self.review_native_preview("background-fades", document_focus=False)
        if self.capture_review:
            self.capture_viewport("settings")
        self.close_settings()
        self.click("editor.source")
        if self.capture_review:
            self.capture_viewport("main")
        chosen = self.snapshot()["document"]["window_background"]
        self.native("fullscreen", document_title, "true")
        self.wait("fullscreen disables directional blur and transparency", lambda d:
                  d["fullscreen"] and not d["native_background"]["translucent"]
                  and d["native_background"]["blur"] is None
                  and d["native_background"]["blur_fade"] == "none" and d["window_background"] == chosen)
        self.stable_state(lambda d: d["fullscreen"] and not d["native_background"]["translucent"]
                          and d["native_background"]["blur"] is None and d["window_background"] == chosen, frames=16)
        if self.capture_review:
            self.capture_viewport("main")
        self.native("fullscreen", document_title, "false")
        self.wait("fullscreen exit restores chosen directional backgrounds", lambda d:
                  not d["fullscreen"] and d["window_background"] == chosen
                  and background_fades_applied(d, "right", "down", keys=("native_background",)))
        self.settings()
        self.setting_section("appearance", "Appearance")
        self.wait("Settings restores its own directional controller", lambda d:
                  background_fades_applied(d, "right", "down"))
        select("blur", "None")
        select("transparency", "None")
        self.wait("uniform backgrounds restored without editing source or the backend", lambda d:
                  background_fades_applied(d, "none", "none")
                  and d["source_fingerprint"] == source
                  and d["preview_requested_backend"] == before["preview_requested_backend"])
        self.close_settings()
        self.stable_state(lambda d: background_fades_applied(d, "none", "none", keys=("native_background",))
                          and d["source_fingerprint"] == source)

    def context_menu(self):
        with self.exact_math_conversion():
            self._context_menu()

    def _context_menu(self):
        import time
        original = "$x$\n\nBody text here.\n"
        self.focus_document_window()
        before = self.snapshot()["document"]
        fixture = self.directory / "menu.typ"
        opened = before["path"] != str(fixture)
        if opened:
            # Aggregate runs start on another journey's document. Prepare and
            # open our own fixture rather than waiting for a file never loaded.
            fixture.write_text(original)
            self.key(31)
            time.sleep(0.8)
            self.native("ax-press-description", "search")
            self.native("text", fixture.name)
            time.sleep(0.8)
            self.native("ax-press-title", "Open")
        self.wait("menu fixture opened", lambda d: d["path"] == str(fixture), timeout=15)
        self.wait_target("editor.source")
        self.click("editor.source")
        self.wait_target("editor.caret")

        def caret_point():
            target = self.snapshot()["targets"].get("editor.caret")
            if not target or not target["enabled"] or target["age_ms"] > 500:
                raise AssertionError("missing, disabled or stale hit target: editor.caret")
            return target["x"], target["y"]

        # Home the caret, then step into the math on line 1. Keyboard
        # navigation from a known state keeps the click point deterministic.
        self.key(126, "cmd")
        self.key(124, "none")
        self.key(124, "none")
        self.wait_target("editor.caret")
        math_point = caret_point()
        self.right_click_at(*math_point)
        self.wait_target("menu.Convert to display math")
        self.click("menu.Convert to display math")
        converted = "$\nx\n$\n\nBody text here.\n"
        self.wait("display-math conversion rewrites the source",
                  lambda d: d["source_fingerprint"] == fingerprint(converted), timeout=15)
        self.key(6)
        self.wait("undo restores the inline math",
                  lambda d: d["source_fingerprint"] == fingerprint(original), timeout=15)

        # A plain-text line offers the standard rows but no Convert row. The
        # absent target can only be asserted once the new menu has painted
        # fresh rows while the old entry keeps aging.
        self.key(126, "cmd")
        self.key(125, "none")
        self.key(125, "none")
        self.wait_target("editor.caret")
        self.right_click_at(*caret_point())
        # Paste is always enabled, unlike Undo right after an undo.
        self.wait_target("menu.Paste")
        deadline = time.monotonic() + 6
        while True:
            state = self.snapshot()
            paste = state["targets"].get("menu.Paste")
            convert = state["targets"].get("menu.Convert to display math")
            if paste and paste["enabled"] and paste["age_ms"] <= 500 and (
                    convert is None or convert["age_ms"] > 1500):
                break
            if time.monotonic() >= deadline:
                raise AssertionError("plain-text menu kept a fresh Convert row")
            time.sleep(0.05)
        self.key(53, "none")
        self.wait("dismissed menu leaves the source alone",
                  lambda d: d["source_fingerprint"] == fingerprint(original), timeout=15)

        # Selected text survives a right-click for Copy. Double-click the
        # first word on the plain-text line, then right-click the same point:
        # the press lands inside the selection and the Copy row stays enabled.
        self.key(126, "cmd")
        self.key(125, "none")
        self.key(125, "none")
        self.wait_target("editor.caret")
        word_point = caret_point()
        self.double_click_at(*word_point)
        self.right_click_at(*word_point)
        self.wait_target("menu.Copy")
        copy = self.snapshot()["targets"].get("menu.Copy")
        if not copy or not copy["enabled"]:
            raise AssertionError("right-click deselected the word: Copy disabled")
        self.copy_from_menu("Body")

        # Whitespace on both sides makes same-line Typst math display math.
        # The converse action must follow the syntax, not newline presence.
        display = "$ x $\n\nBody text here.\n"
        self.click("editor.source")
        self.key(0)
        self.paste_source(display)
        self.wait("same-line display fixture replaces only the source",
                  lambda d: d["source_fingerprint"] == fingerprint(display), timeout=15)
        self.key(126, "cmd")
        self.key(124, "none")
        self.key(124, "none")
        self.wait_target("editor.caret")
        self.right_click_at(*caret_point())
        self.wait_target("menu.Convert to inline math")
        self.click("menu.Convert to inline math")
        self.wait("same-line display math converts to inline math",
                  lambda d: d["source_fingerprint"] == fingerprint(original), timeout=15)

        if opened:
            self.close_scratch(before["tabs"])
            if before["active_tab_id"] is not None:
                self.click("tab." + str(before["active_tab_id"]))

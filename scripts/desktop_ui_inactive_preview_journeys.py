"""Tinymist must finish requested rendering while its document is inactive."""
import time

INACTIVE_PREVIEW_JOURNEYS = ('inactive_preview',)


def inactive_preview_owner(state, before):
    document = state['document']
    if any(document[key] != before[key] for key in ('viewport', 'path', 'source_fingerprint')):
        raise AssertionError('Inactive preview rendering changed its source owner')
    return document


def inactive_tinymist_ready(state, backend, before):
    document = inactive_preview_owner(state, before)
    effective = {'Interactive': 'Tinymist', 'InteractivePartial': 'Tinymist partial'}[backend]
    if document.get('preview_failure'):
        raise AssertionError('Inactive Tinymist failed: ' + str(document['preview_failure']))
    renderer = state.get('renderer')
    value = renderer.get('value') if isinstance(renderer, dict) else None
    return (document['focused'] is False
            and document['preview_requested_backend'] == backend
            and document['preview_backend'] == effective
            and document['preview_native_ready'] and document['preview_surface_exists']
            and isinstance(renderer, dict) and renderer.get('age_ms', 1000) < 500
            and renderer.get('viewport') == before['viewport']
            and isinstance(value, dict) and value.get('loaded') is True
            and value.get('ready') is True
            and value.get('partial') is (backend == 'InteractivePartial'))


class InactivePreviewJourneys:
    def assert_background_owner(self, finder):
        foreground = self.native('foreground')
        if foreground['pid'] == finder:
            return
        if foreground['pid'] == self.process.pid:
            raise AssertionError('Background Tinymist rendering stole application activation')
        raise AssertionError('Background preview check interrupted by foreground change to '
                             + str(foreground.get('bundle', 'another application')))

    def wait_preview_owner(self, before, description, predicate, timeout=60):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            state = self.snapshot(viewport=before['viewport'])
            if predicate(inactive_preview_owner(state, before)):
                return state
            time.sleep(.05)
        raise AssertionError('Timed out: ' + description)

    def focus_preview_owner(self, before, title):
        self.native('activate')
        self.native('raise', title)
        self.native_wait('The exact preview owner has native focus', lambda windows:
                         any(window['AXTitle'] == title and window.get('AXFocused')
                             for window in windows))
        self.wait_preview_owner(before, 'The bound document has keyboard focus',
                                lambda document: document['focused'] is True, timeout=6)

    def open_inactive_preview_settings(self):
        # Settings is process-global and hosted by the primary document, even
        # when the command is issued from a secondary document window.
        self.settings()
        self.setting_section('backend', 'Typst preview backend')
        windows = self.native('windows')['windows']
        return next(window['AXTitle'] for window in windows
                    if 'Settings' in window['AXTitle'] and window.get('AXFocused'))

    def close_inactive_preview_settings(self, before, title, settings_title):
        self.native('activate')
        self.native('raise', settings_title)
        self.native_wait('Global Settings retains native window ownership', lambda windows:
                         any(window['AXTitle'] == settings_title and window.get('AXFocused')
                             for window in windows))
        self.native('close', settings_title)
        self.native_wait('Global Settings is closed', lambda windows:
                         not any(window['AXTitle'] == settings_title for window in windows))
        self.focus_preview_owner(before, title)

    def wait_inactive_tinymist(self, backend, before, finder, timeout=60):
        started = time.monotonic()
        deadline = started + timeout
        while time.monotonic() < deadline:
            # Observation never reactivates the app, retries stolen focus, or
            # accepts a sibling's document/DOM snapshot as this owner's pages.
            self.assert_background_owner(finder)
            state = self.snapshot(viewport=before['viewport'])
            if inactive_tinymist_ready(state, backend, before):
                elapsed_ms = round((time.monotonic() - started) * 1000, 3)
                self.record('inactive_preview.ready', backend=backend,
                            viewport=state['document']['viewport'], elapsed_ms=elapsed_ms,
                            renderer_viewport=state['renderer']['viewport'],
                            partial=state['renderer']['value']['partial'])
                return state
            time.sleep(.075)
        final = self.snapshot(viewport=before['viewport'])['document']
        self.record('inactive_preview.timeout', backend=backend,
                    effective=final.get('preview_backend'), surface=final.get('preview_surface_exists'),
                    ready=final.get('preview_native_ready'), focused=final.get('focused'),
                    failure=final.get('preview_failure'))
        raise AssertionError('Tinymist did not render while its document window was inactive')

    def inactive_preview_window(self, owner, before, title):
        self.focus_preview_owner(before, title)
        for backend in ('Interactive', 'InteractivePartial'):
            settings_title = self.open_inactive_preview_settings()
            self.wait_target('settings.backend.Pdfium')
            self.click('settings.backend.Pdfium')
            self.wait_preview_owner(before, 'The previous native surface is retired', lambda document:
                                    document['preview_requested_backend'] == 'Pdfium'
                                    and not document['preview_surface_exists'])
            self.wait_target('settings.backend.' + backend)
            self.click('settings.backend.' + backend)
            # Keep global Settings open, so even a fast provider constructs
            # the document's child view without that document owning focus.
            finder = self.native('finder')['pid']
            deadline = time.monotonic() + 6
            while self.native('foreground')['pid'] != finder:
                if time.monotonic() >= deadline:
                    raise AssertionError('Finder did not become foreground for the background render')
                time.sleep(.05)
            ready = self.wait_inactive_tinymist(backend, before, finder)
            self.record('inactive_preview.owner', owner=owner, viewport=ready['document']['viewport'])
            for _ in range(8):
                self.assert_background_owner(finder)
                state = self.snapshot(viewport=before['viewport'])
                if not inactive_tinymist_ready(state, backend, before):
                    raise AssertionError('A settled inactive Tinymist preview lost its rendered pages')
                time.sleep(.05)
            windows = self.native('windows')['windows']
            if not any(window['AXTitle'] == settings_title and not window.get('AXMinimized')
                       for window in windows):
                raise AssertionError('Global Settings disappeared during the background render')
            self.assert_background_owner(finder)
            self.close_inactive_preview_settings(before, title, settings_title)
        settings_title = self.open_inactive_preview_settings()
        self.wait_target('settings.backend.Pdfium')
        self.click('settings.backend.Pdfium')
        self.wait_preview_owner(before, 'Native preview is retired before the next owner', lambda document:
                                document['preview_requested_backend'] == 'Pdfium'
                                and not document['preview_surface_exists'])
        self.close_inactive_preview_settings(before, title, settings_title)

    def inactive_preview(self):
        from desktop_ui_journeys import PREVIEW_SOURCE
        self.focus_document_window()
        self.ensure_typesetting_tab()
        initial = self.snapshot()['document']
        original_title = next(window['AXTitle'] for window in self.native('windows')['windows']
                              if window.get('AXFocused') and 'Settings' not in window['AXTitle'])
        self.inactive_preview_window('root', initial, original_title)
        self.multiple_windows = True
        self.key(45, 'cmd+shift')
        windows = self.native_wait('A secondary document window opens', lambda windows:
                                  any(window.get('AXFocused') and window['AXTitle'] != original_title
                                      for window in windows))
        secondary_title = next(window['AXTitle'] for window in windows if window.get('AXFocused'))
        self.native('raise', secondary_title)
        self.viewport = None
        secondary = self.directory / 'inactive-secondary.typ'
        secondary.write_text(PREVIEW_SOURCE, encoding='utf-8')
        self.open_harper_fixture(secondary)
        opened = self.wait('Secondary owns the independent Typst fixture', lambda document:
                           document['path'] == str(secondary) and document['viewport'] != initial['viewport'])
        secondary_title = secondary.name + ' — tiptoptyp'
        self.inactive_preview_window('secondary', opened['document'], secondary_title)
        self.native('close', secondary_title)
        self.native('raise', original_title)
        self.multiple_windows = False
        self.viewport = None
        restored = self.wait_preview_owner(initial, 'The original document is restored unchanged',
                                           lambda document: document['focused'] is True
                                           and document['minimized'] is not True)
        self.record('inactive_preview.restored', viewport=restored['document']['viewport'])

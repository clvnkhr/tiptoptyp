"""Native image appearance journey; only private generated image files are used."""

import hashlib
import struct
import time
import zlib

IMAGE_JOURNEYS = ("images",)
ORIGINAL_PIXELS = [255, 255, 255, 255, 0, 0, 0, 255,
                   255, 0, 0, 128, 32, 64, 96, 0]


def image_fixture():
    """A white drawing with color/alpha swatches and four diagnostic pixels."""
    width, height = 520, 340
    rows = []
    for y in range(height):
        row = bytearray()
        for x in range(width):
            if y == 0 and x < 4:
                pixel = ORIGINAL_PIXELS[x * 4:x * 4 + 4]
            elif 28 <= x < 492 and 28 <= y < 312:
                border = x < 32 or x >= 488 or y < 32 or y >= 308
                if border:
                    pixel = (0, 0, 0, 255)
                elif 55 <= x < 190 and 75 <= y < 265:
                    pixel = (20, 60, 150, 255)
                elif 215 <= x < 330 and 75 <= y < 265:
                    pixel = (200, 60, 30, 128)
                elif 355 <= x < 465 and 75 <= y < 265:
                    pixel = (30, 120, 70, 0)
                else:
                    pixel = (255, 255, 255, 255)
            else:
                pixel = (255, 255, 255, 255)
            row.extend(pixel)
        rows.append(b"\x00" + row)

    def chunk(kind, payload):
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload) & 0xffffffff))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"".join(rows)))
            + chunk(b"IEND", b""))


def dark_pixels(pixels):
    result = list(pixels)
    for start in range(0, len(result), 4):
        for channel, percent in enumerate((92, 94, 100)):
            result[start + channel] = (255 - pixels[start + channel]) * percent // 100
    return result


class ImageJourneys:
    def image_page_choice(self, name):
        self.settings()
        if self.snapshot()["document"]["settings_json"]["visible"]:
            self.click("settings.json.mode")
        self.setting_section("page", "Page")
        self.wait_target("settings.page." + name)
        self.click("settings.page." + name)
        if name != "FollowInterface":
            self.wait("page appearance is " + name,
                      lambda d: d["page_dark"] == (name == "Dark"))
        self.close_settings()

    def images(self):
        self.focus_document_window()
        original_document = self.snapshot()["document"]
        self.image_page_choice("Dark")
        # Defaults preserve original image colors even when PDFs use dark pages.
        if self.snapshot()["document"]["dark_images"]:
            raise AssertionError("image dark mode must start disabled")
        path = self.directory / "image-appearance.png"
        encoded = image_fixture()
        path.write_bytes(encoded)
        checksum = hashlib.sha256(encoded).hexdigest()
        self.key(31)
        time.sleep(0.8)
        self.key(5, "cmd+shift")
        time.sleep(0.3)
        self.key(0)
        self.native("text", str(path))
        self.key(36, "none")
        time.sleep(0.5)
        self.native("ax-press-title", "Open")
        self.wait("private image file opens", lambda d: d["path"] == str(path), timeout=15)

        def settled(dark):
            expected = dark_pixels(ORIGINAL_PIXELS) if dark else ORIGINAL_PIXELS
            return lambda d: (d["path"] == str(path) and d["asset_image_ready"]
                              and d["asset_preview_dark"] == dark
                              and d["asset_image_pixels"] == expected)

        def verify(dark, phase):
            self.wait("image pixels settle: " + phase, settled(dark), timeout=15)
            self.stable_state(settled(dark))
            if hashlib.sha256(path.read_bytes()).hexdigest() != checksum:
                raise AssertionError("image appearance changed the original file")
            self.record("image.appearance", phase=phase, dark=dark, source_sha256=checksum)
            if self.capture_review and phase in (
                    "original colors on dark pages", "dark toggle 0",
                    "light pages keep original colors"):
                self.capture_viewport("main")

        verify(False, "original colors on dark pages")
        for cycle in range(2):
            self.wait_target("image.dark")
            self.click("image.dark")
            self.wait("image dark preference enables", lambda d: d["dark_images"])
            verify(True, "dark toggle " + str(cycle))
            self.click("image.dark")
            self.wait("image dark preference disables", lambda d: not d["dark_images"])
            verify(False, "original toggle " + str(cycle))
        self.click("image.dark")
        self.wait("opt in for page mode transitions", lambda d: d["dark_images"])
        verify(True, "dark pages opted in")
        self.image_page_choice("Light")
        verify(False, "light pages keep original colors")
        self.stable_state(lambda d: d["dark_images"] and not d["asset_preview_dark"])
        self.image_page_choice("Dark")
        verify(True, "dark pages restore inversion")
        self.click("image.dark")
        self.wait("restore image opt-in default", lambda d: not d["dark_images"])
        verify(False, "restored default")
        self.image_page_choice("FollowInterface")
        verify(False, "follow interface with images opted out")
        self.key(13)
        self.wait("image closes without changing the prior document",
                  lambda d: d["tabs"] == original_document["tabs"]
                  and d["path"] == original_document["path"]
                  and d["source_fingerprint"] == original_document["source_fingerprint"])

"""Regenerate the RecordScreen window and Windows executable icons."""

from pathlib import Path

from PIL import Image, ImageDraw


OUT = Path(__file__).parent
SIZE = 512
image = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(image)

# Blue app tile, screen frame, and red recording indicator.
draw.rounded_rectangle((24, 24, 488, 488), radius=112, fill="#15243b")
draw.rounded_rectangle((87, 123, 425, 347), radius=35, fill="#f5f8fc")
draw.rounded_rectangle((108, 144, 404, 326), radius=19, fill="#283f5f")
draw.rounded_rectangle((215, 350, 297, 380), radius=9, fill="#f5f8fc")
draw.rounded_rectangle((170, 377, 342, 400), radius=11, fill="#f5f8fc")
draw.ellipse((304, 101, 446, 243), fill="#15243b")
draw.ellipse((319, 116, 431, 228), fill="#ff4c53")

image.save(OUT / "recordscreen.png")
image.save(
    OUT / "recordscreen.ico",
    sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
)

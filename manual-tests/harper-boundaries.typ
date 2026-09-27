#set page(margin: 1in)
#set text(lang: "en", region: "gb", font: "NonsensicalFontName")

// Ordinary prose: these should be reported by Harper.
= Harper Typst boundary fixture
This sentnce has a typo and a repeated repeated word.

// Inline math: the prose on both sides should remain one sentence.
This sentnce continues across inline math $x^2 + y^2 = z^2$ and ends here.

// A display equation uses the same delimiter, on its own block.
$
  x^2 + y^2 = z^2
$

// Named technical arguments and strings should be protected.
#let technical_fn(name, value: "NonsensicalTechnicalToken") = name
#technical_fn("NonsensicalArgumentToken", value: "NonsensicalValueToken")
#set text(lang: "en", region: "gb", font: "AnotherTechnicalFontName")

// Text-bearing arguments remain prose and should be checked.
#text("This sentnce is inside a text function.")
#link("https://example.invalid/NonsensicalToken", "This sentnce is link text.")

// Capitalized names should not be reported as spelling errors.
The Leray theorem is discussed here.

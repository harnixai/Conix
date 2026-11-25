import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from textkit import slugify, word_count  # noqa: E402


class SlugifyTest(unittest.TestCase):
    def test_lowercases_and_joins_with_hyphens(self):  # TK-R1
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_strips_leading_and_trailing_hyphens(self):  # TK-R1
        self.assertEqual(slugify("  --Rust & Python--  "), "rust-python")


class WordCountTest(unittest.TestCase):
    def test_counts_whitespace_separated_words(self):  # TK-R2
        self.assertEqual(word_count("one  two\nthree"), 3)

    def test_empty_text_has_no_words(self):  # TK-R2
        self.assertEqual(word_count("   "), 0)


if __name__ == "__main__":
    unittest.main()

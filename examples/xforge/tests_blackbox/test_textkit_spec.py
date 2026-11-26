"""textkit 的黑盒测试：只照 xforge/specs/textkit.md 的 WHEN / THEN、只经公开接口（xforge/interfaces/textkit.yaml），
不读 src/textkit 的实现。xforge/dual 的测试那一路往这个目录里加；verify 菜单的 blackbox 一项跑它。"""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

import textkit  # noqa: E402


class SpecTest(unittest.TestCase):
    def test_slugify_lowercases_and_hyphenates(self):  # TK-R1 fn:textkit.slugify
        self.assertEqual(textkit.slugify("Hello, World!"), "hello-world")

    def test_slugify_trims_hyphens(self):  # TK-R1
        self.assertEqual(textkit.slugify("--a--"), "a")

    def test_word_count_counts_whitespace_separated(self):  # TK-R2 fn:textkit.word_count
        self.assertEqual(textkit.word_count("a b\tc\n d"), 4)

    def test_word_count_of_blank_is_zero(self):  # TK-R2
        self.assertEqual(textkit.word_count(" \n\t "), 0)


if __name__ == "__main__":
    unittest.main()

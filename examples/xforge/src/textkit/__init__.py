"""textkit：几个处理文本的小函数。演示项目里被"变更流程"改的那份代码。"""

import re

__all__ = ["slugify", "word_count"]


def slugify(text: str) -> str:
    """把一段文字变成 URL 里用的短名：小写，非字母数字的连续字符换成一个连字符，去掉首尾的连字符。"""
    s = re.sub(r"[^0-9a-zA-Z]+", "-", text.lower())
    return s.strip("-")


def word_count(text: str) -> int:
    """数英文单词：以空白分隔的非空片段个数。"""
    return len(text.split())

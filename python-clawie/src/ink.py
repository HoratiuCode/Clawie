from __future__ import annotations

import json
import textwrap


def render_markdown_panel(text: str) -> str:
    border = '=' * 40
    return f"{border}\n{text}\n{border}"


def render_code_block(text: str, language: str = '') -> str:
    body = textwrap.dedent(text).strip('\n')
    if not body:
        return '```'
    fence = '````' if '```' in body else '```'
    suffix = f'{language}\n' if language else '\n'
    return f'{fence}{suffix}{body}\n{fence}'


def render_smooth_output(text: str, language: str = '') -> str:
    body = textwrap.dedent(text).strip()
    if not body:
        return body

    inferred_language = language
    if not inferred_language:
        try:
            json.loads(body)
        except json.JSONDecodeError:
            inferred_language = 'text' if '\n' in body else ''
        else:
            inferred_language = 'json'

    if '\n' in body or inferred_language:
        return render_code_block(body, inferred_language)
    return body

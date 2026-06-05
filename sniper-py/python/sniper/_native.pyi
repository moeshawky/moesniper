from typing import Any


def sniper_edit(
    filepath: str, start: int, end: int, content: str
) -> dict[str, Any]: ...


def sniper_delete(filepath: str, start: int, end: int) -> dict[str, Any]: ...


def sniper_manifest(filepath: str, operations_json: str) -> dict[str, Any]: ...


def sniper_undo(filepath: str) -> str: ...


def sniper_encode(text: str) -> str: ...


def sniper_decode(hex_str: str) -> str: ...


def sniper_read_file(filepath: str) -> str: ...


def sniper_config() -> dict[str, Any]: ...

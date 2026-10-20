"""The restricted version-1 JSON domain and RFC 8785 encoding."""

import json
import math

MAX_SAFE_INTEGER = (1 << 53) - 1


class CanonicalJsonError(ValueError):
    """A value cannot be represented by Metewand's canonical JSON contract."""


def _normalize(value):
    if value is None or type(value) is bool:
        return value
    if type(value) is str:
        value.encode("utf-8")
        return value
    if type(value) is int:
        if abs(value) > MAX_SAFE_INTEGER:
            raise CanonicalJsonError(
                "Integers must lie in the inclusive 53-bit safe range."
            )
        return value
    if type(value) is float:
        if not math.isfinite(value):
            raise CanonicalJsonError("Numbers must be finite binary64 values.")
        if value.is_integer():
            return _normalize(int(value))
        return value
    if type(value) is list:
        return [_normalize(item) for item in value]
    if type(value) is dict:
        if any(type(key) is not str for key in value):
            raise CanonicalJsonError("Object keys must be strings.")
        return {_normalize(key): _normalize(item) for key, item in value.items()}
    raise CanonicalJsonError(f"Unsupported JSON value type: {type(value).__name__}.")


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise CanonicalJsonError(f"Duplicate JSON object key: {key!r}.")
        result[key] = value
    return result


def _invalid_constant(value):
    raise CanonicalJsonError(f"Invalid JSON number: {value}.")


def parse_json(source):
    """Parse UTF-8 bytes or text, rejecting duplicate keys and noncanonical values.

    The returned value normalizes integral floats and negative zero to integers.
    Source whitespace and property order need not be canonical.
    """
    try:
        if isinstance(source, bytes):
            source = source.decode("utf-8")
        if not isinstance(source, str):
            raise CanonicalJsonError("JSON input must be UTF-8 bytes or text.")
        return _normalize(
            json.loads(
                source,
                object_pairs_hook=_unique_object,
                parse_constant=_invalid_constant,
            )
        )
    except (ValueError, UnicodeError, RecursionError) as error:
        raise CanonicalJsonError(str(error)) from error


def _float_text(value):
    # Python supplies the shortest round-tripping digits; ECMAScript uses a
    # different threshold for exponent notation and omits exponent zero padding.
    text = repr(value)
    if "e" not in text:
        return text
    mantissa, exponent = text.split("e")
    exponent = int(exponent)
    if abs(value) < 1e-6:
        return f"{mantissa}e{exponent}"
    negative = mantissa.startswith("-")
    digits = mantissa.lstrip("-").replace(".", "")
    point = 1 + exponent
    if point <= 0:
        text = "0." + "0" * -point + digits
    elif point >= len(digits):
        text = digits + "0" * (point - len(digits))
    else:
        text = digits[:point] + "." + digits[point:]
    return ("-" if negative else "") + text


def _encode(value):
    if value is None:
        return "null"
    if type(value) is bool:
        return "true" if value else "false"
    if type(value) is str:
        return json.dumps(value, ensure_ascii=False)
    if type(value) is int:
        return str(value)
    if type(value) is float:
        return _float_text(value)
    if type(value) is list:
        return "[" + ",".join(_encode(item) for item in value) + "]"
    keys = sorted(value, key=lambda key: key.encode("utf-16-be"))
    return (
        "{" + ",".join(_encode(key) + ":" + _encode(value[key]) for key in keys) + "}"
    )


def canonical_bytes(value):
    """Encode built-in JSON values as canonical UTF-8 bytes without a newline.

    This never coerces tuples, arrays, custom scalars, or nonstring object keys.
    Convert library-specific values explicitly in benchmark code.
    """
    try:
        return _encode(_normalize(value)).encode("utf-8")
    except (ValueError, UnicodeError, RecursionError) as error:
        raise CanonicalJsonError(str(error)) from error

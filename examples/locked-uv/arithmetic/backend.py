"""Build a deterministic example wheel without external build dependencies."""

from pathlib import Path
import zipfile


def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    name = "metewand_example_arithmetic-1.0.0"
    files = {
        "metewand_example_arithmetic.py": Path("metewand_example_arithmetic.py").read_text(),
        f"{name}.dist-info/METADATA": "Metadata-Version: 2.3\nName: metewand-example-arithmetic\nVersion: 1.0.0\nRequires-Python: >=3.11\n",
        f"{name}.dist-info/WHEEL": "Wheel-Version: 1.0\nGenerator: metewand-example\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
    }
    files[f"{name}.dist-info/RECORD"] = "".join(f"{path},,\n" for path in files) + f"{name}.dist-info/RECORD,,\n"
    filename = f"{name}-py3-none-any.whl"
    with zipfile.ZipFile(Path(wheel_directory) / filename, "w") as wheel:
        for path, content in sorted(files.items()):
            entry = zipfile.ZipInfo(path)
            entry.external_attr = 0o644 << 16
            wheel.writestr(entry, content)
    return filename

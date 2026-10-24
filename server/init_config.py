"""Create a local writable pairing config without overwriting an existing one."""
import argparse
from pathlib import Path


def main():
    base = Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--path", type=Path, default=base.parent / "data/config.py")
    target = parser.parse_args().path
    target.parent.mkdir(parents=True, exist_ok=True)
    try:
        with target.open("xb") as output:
            output.write((base / "config.example.py").read_bytes())
    except FileExistsError:
        if not target.is_file():
            raise RuntimeError(f"config target is not a file: {target}")
        print(f"Existing configuration preserved: {target}")
    else:
        print(f"Created blank pairing configuration: {target}")


if __name__ == "__main__":
    main()

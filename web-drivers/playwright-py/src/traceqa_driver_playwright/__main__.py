from traceqa_protocol.rpc import run_driver

from .driver import PlaywrightDriver


def main() -> None:
    run_driver(PlaywrightDriver)


if __name__ == "__main__":
    main()

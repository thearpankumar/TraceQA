from traceqa_protocol.rpc import run_driver

from .driver import WebDriverDriver


def main() -> None:
    run_driver(WebDriverDriver)


if __name__ == "__main__":
    main()

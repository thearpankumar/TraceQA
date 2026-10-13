from traceqa_protocol import models
from traceqa_protocol.rpc import Channel

from .driver import PlaywrightDriver


def main() -> None:
    channel = Channel.from_stdio()
    driver = PlaywrightDriver(channel)
    try:
        channel.serve(driver.handlers())
    finally:
        # Also reached when the engine goes away without saying goodbye. Closing twice is harmless.
        driver.shutdown(models.Empty())


if __name__ == "__main__":
    main()

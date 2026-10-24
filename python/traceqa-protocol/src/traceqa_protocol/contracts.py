# Generated from spec/ by scripts/gen_python_protocol.py. Do not edit.

"""Contract versions and method names."""

CLIENT_PROTOCOL_VERSION = 1
CLIENT_PROTOCOL_METHODS = {
    "engine.hello": {
        "params": "EngineHelloParams",
        "result": "EngineHelloResult"
    },
    "run.start": {
        "params": "RunStartParams",
        "result": "RunStartResult"
    },
    "run.cancel": {
        "params": "RunCancelParams",
        "result": "Empty"
    },
    "engine.shutdown": {
        "params": "Empty",
        "result": "Empty"
    }
}
CLIENT_PROTOCOL_NOTIFICATIONS = {
    "run.event": {
        "params": "RunEventParams"
    },
    "run.done": {
        "params": "RunDoneParams"
    }
}

COLLECTOR_VERSION = 1

COMMON_VERSION = 1

DRIVER_PROTOCOL_VERSION = 1
DRIVER_PROTOCOL_METHODS = {
    "driver.hello": {
        "params": "DriverHelloParams",
        "result": "DriverHelloResult"
    },
    "driver.shutdown": {
        "params": "Empty",
        "result": "Empty"
    },
    "session.open": {
        "params": "SessionOpenParams",
        "result": "SessionOpenResult"
    },
    "session.close": {
        "params": "SessionCloseParams",
        "result": "Empty"
    },
    "session.storage": {
        "params": "SessionStorageParams",
        "result": "StorageState"
    },
    "page.goto": {
        "params": "PageGotoParams",
        "result": "PageGotoResult"
    },
    "page.eval": {
        "params": "PageEvalParams",
        "result": "PageEvalResult"
    },
    "page.scroll": {
        "params": "PageScrollParams",
        "result": "PageScrollResult"
    },
    "page.screenshot": {
        "params": "PageScreenshotParams",
        "result": "PageScreenshotResult"
    },
    "page.act": {
        "params": "PageActParams",
        "result": "PageActResult"
    }
}
DRIVER_PROTOCOL_NOTIFICATIONS = {
    "driver.log": {
        "params": "DriverLogParams"
    }
}

RUN_FOLDER_VERSION = 1

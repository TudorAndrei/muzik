import sys

import pytest

from muzik.core.runner import run_streaming
from muzik.core.workflow.cancellation import CancellationToken, WorkflowCancelled


def test_run_streaming_terminates_owned_process_when_cancelled() -> None:
    token = CancellationToken()
    token.cancel()

    with pytest.raises(WorkflowCancelled):
        run_streaming(
            [sys.executable, "-c", "import time; time.sleep(30)"],
            cancellation=token,
        )

import tempfile
import unittest
from pathlib import Path

from metewand import OneShotImplementation, WorkerError
from metewand._worker import _Dispatcher


class Controlled(OneShotImplementation):
    def prepare(self, *args):
        self.state = 0

    def execute(self, budget, observation_control):
        assert budget is None
        self.state += observation_control["iterations"]
        observation_control["iterations"] = 99
        return {"answer": self.state}


class FreshTests(unittest.TestCase):
    def test_controls_are_forwarded_and_echo_is_frozen(self):
        worker = Controlled()
        dispatcher = _Dispatcher(worker, "implementation", fresh_sequence=True)
        hello = dispatcher.dispatch({"id":"0", "method":"hello", "protocols":[1], "role":"implementation", "worker_id":"mw1-resolved-worker-" + "a" * 64})
        self.assertIn("fresh_sequence", hello["capabilities"])
        worker.prepare()
        dispatcher.prepared = True
        with tempfile.TemporaryDirectory() as directory:
            reply = dispatcher.dispatch({"id":"1", "method":"execute", "result_dir":directory, "observation_control":{"iterations":4}})
            self.assertEqual(reply["observation_control"], {"iterations":4})
            self.assertTrue((Path(directory) / reply["result"]["manifest"]).is_file())
        self.assertEqual(worker.state, 4)

    def test_legacy_workers_reject_controls(self):
        dispatcher = _Dispatcher(Controlled(), "implementation")
        dispatcher.ready = True
        dispatcher.prepared = True
        with self.assertRaises(WorkerError):
            dispatcher.dispatch({"id":"1", "method":"execute", "result_dir":"unused", "observation_control":{"iterations":4}})

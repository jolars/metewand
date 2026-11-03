import tempfile
import unittest
from pathlib import Path

from binding_support import exchange, hello, request
from metewand_adapter._artifacts import read_json


class FreshTests(unittest.TestCase):
    def test_warmup_runs_once_and_measured_state_is_reconstructed(self):
        from metewand_adapter._bindings import _Callbacks

        events = []

        def prepare(context):
            state = {"answer": 0}
            events.append(("prepare", state))
            return state

        def warmup(state, context):
            state["answer"] = 99
            events.append(("warmup", state))

        worker = _Callbacks(
            {
                "prepare": prepare,
                "warmup": warmup,
                "reset": lambda state, context: events.append(("reset", state)),
            }
        )
        worker.prepare()
        self.assertEqual(worker.state, {"answer": 0})
        self.assertEqual(
            [name for name, _ in events], ["prepare", "warmup", "reset", "prepare"]
        )
        worker.reset()
        worker.prepare()
        self.assertEqual(sum(name == "warmup" for name, _ in events), 1)
        self.assertIsNot(events[0][1], worker.state)

    def test_controls_are_forwarded_and_echo_is_frozen(self):
        state = {"answer": 0}

        def execute(prepared, context):
            self.assertIs(prepared, state)
            self.assertIsNone(context.budget)
            state["answer"] += context.observation_control["iterations"]
            context.observation_control["iterations"] = 99
            return state

        with tempfile.TemporaryDirectory() as directory:
            replies = exchange(
                {"prepare": lambda context: state, "execute": execute},
                "implementation",
                [
                    hello(),
                    request(
                        "prepare",
                        dataset_dir=directory,
                        problem_contract_id="contract",
                        problem_parameters={},
                        implementation_parameters={},
                        implementation_seed=7,
                    ),
                    request(
                        "execute",
                        result_dir=directory,
                        observation_control={"iterations": 4},
                    ),
                    request("shutdown"),
                ],
            )
            self.assertIn("fresh_sequence", replies[0]["capabilities"])
            self.assertEqual(replies[2]["observation_control"], {"iterations": 4})
            self.assertEqual(
                read_json(Path(directory) / replies[2]["draft"])["data"], {"answer": 4}
            )

    def test_invalid_controls_do_not_consume_preparation(self):
        with tempfile.TemporaryDirectory() as directory:
            replies = exchange(
                {"prepare": lambda context: None, "execute": lambda state, context: {}},
                "implementation",
                [
                    hello(),
                    request(
                        "prepare",
                        dataset_dir=directory,
                        problem_contract_id="contract",
                        problem_parameters={},
                        implementation_parameters={},
                        implementation_seed=7,
                    ),
                    *[
                        request(
                            "execute", result_dir=directory, observation_control=control
                        )
                        for control in [None, [], True]
                    ],
                    request("execute", result_dir=directory, observation_control={}),
                    request("shutdown"),
                ],
            )
            for response in replies[2:5]:
                self.assertEqual(response["error"]["code"], "invalid_request")
            self.assertTrue(replies[5]["ok"])

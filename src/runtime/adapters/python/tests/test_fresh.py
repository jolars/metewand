import tempfile
import unittest
from pathlib import Path

from binding_support import exchange, hello, request
from metewand_adapter._artifacts import read_json


class FreshTests(unittest.TestCase):
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

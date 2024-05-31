import collections
import math
import sys
import os
import pprint
from typing import Iterator, Any

from tqdm import tqdm
import torch
from torch import nn
from torch.backends import cudnn, cuda

from text_utils import (
    api,
    logging,
    configuration,
    io,
    data,
)
from text_utils.api.utils import Device, get_devices

__all__ = ["ModelInfo"]

ModelInfo = collections.namedtuple(
    "ModelInfo",
    ["name", "description", "tags"]
)


class TextProcessor:
    task: str
    pretrained: bool = False
    devices: list[torch.device]

    @classmethod
    def _task_upper(cls) -> str:
        return cls.task.upper()

    @classmethod
    def available_models(cls) -> list[ModelInfo]:
        raise NotImplementedError

    @classmethod
    def default_model(cls) -> ModelInfo | None:
        available_models = cls.available_models()
        if len(available_models) == 0:
            return None
        for info in available_models:
            if "default" in info.tags:
                return info
        return available_models[0]

    @classmethod
    def _model_url(cls, model: str) -> str:
        raise NotImplementedError

    @classmethod
    def download_dir(cls) -> str:
        task_name = cls._task_upper().replace(" ", "_")
        return os.environ.get(
            f"{task_name}_DOWNLOAD_DIR",
            os.path.join(
                os.path.dirname(__file__),
                ".download",
                task_name
            )
        )

    @classmethod
    def cache_dir(cls) -> str:
        task_name = cls._task_upper().replace(" ", "_")
        return os.environ.get(
            f"{task_name}_CACHE_DIR",
            os.path.join(
                os.path.dirname(__file__),
                ".cache",
                task_name
            )
        )

    @classmethod
    def from_pretrained(
        cls,
        model: str | None = None,
        device: Device = "cuda",
        download_dir: str | None = None,
        cache_dir: str | None = None,
        force_download: bool = False
    ):
        if model is None:
            default = cls.default_model()
            assert default is not None, "no default model available"
            model = default.name

        assert model is not None
        assert any(model == m.name for m in cls.available_models()), \
            f"model {model} does not match any of the available models:\n" \
            f"{pprint.pformat(cls.available_models())}"

        logger = logging.get_logger(f"{cls._task_upper()} DOWNLOAD")
        model_url = cls._model_url(model)
        if download_dir is None:
            download_dir = cls.download_dir()
        if cache_dir is None:
            cache_dir = cls.cache_dir()
        sub_cache_dir = model.lower().replace(" ", "_")
        zip_dir = api.download_zip(
            model,
            model_url,
            download_dir,
            cache_dir,
            sub_cache_dir,
            force_download,
            logger
        )
        sub_dirs = os.listdir(zip_dir)
        assert len(sub_dirs) == 1, \
            f"expected extracted zip for model {model} to contain " \
            f"one subdirectory, but got {len(sub_dirs)}:\n{pprint.pformat(sub_dirs)}"
        # mark processor as pretrained
        cls.pretrained = True
        return cls.from_experiment(os.path.join(zip_dir, sub_dirs[0]), device)

    @classmethod
    def from_experiment(
        cls,
        experiment_dir: str,
        device: Device = "cuda"
    ):
        cfg = configuration.load_config_from_experiment(experiment_dir)
        model = cls._model_from_config(cfg, device)
        best_checkpoint_path = os.path.join(
            experiment_dir,
            "checkpoints",
            "checkpoint_best.pt"
        )
        if not os.path.exists(best_checkpoint_path):
            best_checkpoint_path = os.path.join(
                experiment_dir,
                "checkpoints",
                "checkpoint_last.pt"
            )
        if os.path.exists(best_checkpoint_path):
            best_checkpoint = io.load_checkpoint(best_checkpoint_path)
            model.load_state_dict(best_checkpoint["model_state_dict"])
        model = model.eval().requires_grad_(False)
        return cls(model, cfg, device)

    @property
    def name(self) -> str:
        raise NotImplementedError

    @classmethod
    def _model_from_config(
        cls,
        cfg: dict[str, Any],
        device: Device
    ) -> nn.Module:
        raise NotImplementedError

    @property
    def max_length(self) -> int:
        raise NotImplementedError

    def __init__(
        self,
        model: nn.Module,
        cfg: dict[str, Any],
        device: Device = "cuda"
    ) -> None:
        self.cfg = cfg
        self.logger = logging.get_logger(self._task_upper())
        self.logger.debug(f"got config:\n{self.cfg}")

        torch.set_num_threads(len(os.sched_getaffinity(0)))
        torch.use_deterministic_algorithms(False)
        cudnn.benchmark = True
        cuda.matmul.allow_tf32 = True

        self.model = model
        self.to(device)

    @torch.inference_mode()
    def _inference(self, batch: data.InferenceBatch) -> Iterator[Any]:
        raise NotImplementedError

    def _process_results(
        self,
        items: list[data.InferenceItem],
        outputs: list[Any]
    ) -> data.InferenceData:
        raise NotImplementedError

    def _get_loader(
        self,
        iter: Iterator[data.InferenceData],
        batch_size: int = 16,
        batch_max_tokens: int | None = None,
        sort: bool = True,
        num_threads: int | None = None,
        **kwargs: Any
    ) -> data.InferenceLoader:
        if num_threads is None:
            num_threads = min(len(os.sched_getaffinity(0)), 4)

        if batch_max_tokens is None:
            batch_limit = max(1, batch_size)
            batch_limit_type = "batch_size"
            buffer_size = batch_limit
        else:
            batch_limit = max(batch_max_tokens, self.max_length)
            batch_limit_type = "padded_item_size"
            min_items_per_batch = math.ceil(batch_limit / self.max_length)
            buffer_size = min_items_per_batch

        if sorted:
            prefetch_factor = sys.maxsize
        else:
            prefetch_factor = 1

        inference_cfg = {
            "tokenizer": self.cfg["inference"]["tokenizer"],
            "window": self.cfg["inference"].get("window", {"type": "full"}),
            "num_threads": num_threads,
            "batch_limit": batch_limit,
            "buffer_size": buffer_size,
            "prefetch_factor": prefetch_factor,
            "batch_limit_type": batch_limit_type,
            "sort": sort
        }
        inference_cfg.update(kwargs)
        return data.InferenceLoader.from_iterator(
            iter,
            **inference_cfg
        )

    def _pbar(
        self,
        progress_desc: str,
        progress_total: int,
        progress_unit: str = "seq",
        show_progress: bool = False,
    ) -> tqdm:
        if progress_unit == "seq":
            return api.sequence_progress_bar(progress_desc, progress_total, not show_progress)
        elif progress_unit == "byte":
            return api.byte_progress_bar(progress_desc, progress_total, not show_progress)
        else:
            raise ValueError(
                f"unknown progress unit {progress_unit}, must be either 'seq' or 'byte'"
            )

    def _process_sorted(
        self,
        loader: data.InferenceLoader,
        progress_desc: str,
        progress_total: int,
        progress_unit: str = "seq",
        show_progress: bool = False,
    ) -> list[data.InferenceData]:
        results = {}
        pbar = self._pbar(
            progress_desc,
            progress_total,
            progress_unit,
            show_progress
        )
        for batch in loader:
            # use only last output of inference iterator
            *_, outputs = self._inference(batch)
            for item, output in zip(batch.items(), outputs):
                if item.item_idx not in results:
                    results[item.item_idx] = {}
                    if progress_unit == "seq":
                        pbar.update(1)
                if progress_unit == "byte":
                    pbar.update(item.window_bytes())
                results[item.item_idx][item.window_idx] = (item, output)
        outputs = []
        for item_idx in range(len(results)):
            window_items = []
            window_outputs = []
            for window_idx in range(len(results[item_idx])):
                item, output = results[item_idx][window_idx]
                window_items.append(item)
                window_outputs.append(output)
            outputs.append(self._process_results(window_items, window_outputs))
        return outputs

    def _process_unsorted(
        self,
        loader: data.InferenceLoader,
        progress_desc: str,
        progress_total: int,
        progress_unit: str = "seq",
        show_progress: bool = False,
    ) -> Iterator[data.InferenceData]:
        prev_item_idx = 0
        window_items = []
        window_outputs = []
        pbar = self._pbar(
            progress_desc,
            progress_total,
            progress_unit,
            show_progress
        )
        for batch in loader:
            *_, outputs = self._inference(batch)
            for item, output in zip(batch.items(), outputs):
                if item.item_idx == prev_item_idx:
                    window_items.append(item)
                    window_outputs.append(output)
                    continue
                yield self._process_results(window_items, window_outputs)
                if progress_unit == "seq":
                    pbar.update(1)
                else:
                    pbar.update(sum(
                        item.window_bytes()
                        for item in window_items
                    ))
                prev_item_idx = item.item_idx
                window_items = [item]
                window_outputs = [output]
        # dont forget to yield final item
        yield self._process_results(window_items, window_outputs)

    def to(self, device: Device) -> "TextProcessor":
        self.devices = get_devices(device)
        assert len(self.devices) == 1, \
            "only a single device supported by default, implement custom to() if you need " \
            "multi-device support"
        self.model = self.model.to(self.devices[0])
        return self

import copy
from typing import Dict, Any, List, Optional, Callable

import einops
import torch
from torch import nn


class FocalLoss(nn.Module):
    # copied and modified from https://github.com/AdeelH/pytorch-multi-class-focal-loss/blob/master/focal_loss.py
    def __init__(
        self,
        alpha: Optional[List[float]],
        gamma: float,
        reduction: str = "mean",
        ignore_index: int = -100,
    ):
        super().__init__()
        self.alpha = alpha
        self.gamma = gamma
        self.reduction = reduction
        self.ignore_index = ignore_index
        self.nll_loss = nn.NLLLoss(
            weight=torch.as_tensor(alpha, dtype=torch.float) if alpha is not None else None,
            reduction="none",
            ignore_index=ignore_index
        )

    def forward(self, outputs: torch.Tensor, labels: torch.Tensor) -> torch.Tensor:
        assert outputs.ndim == 2 and labels.ndim == 1
        # make sure outputs and labels have correct types
        outputs = outputs.float()
        labels = labels.long()
        unignored_mask = labels != self.ignore_index
        labels = labels[unignored_mask]
        if len(labels) == 0:
            return torch.tensor(0, device=outputs.device, dtype=torch.float)
        outputs = outputs[unignored_mask]

        log_p = torch.log_softmax(outputs, dim=-1)
        ce = self.nll_loss(log_p, labels)

        log_pt = log_p[torch.arange(len(outputs), device=outputs.device), labels]
        pt = log_pt.exp()
        focal_term = torch.pow((1 - pt).clamp(0, 1), self.gamma)
        ce = focal_term * ce

        if self.reduction == "mean":
            ce = ce.mean()
        elif self.reduction == "sum":
            ce = ce.sum()
        return ce


class SeqLoss(nn.Module):
    """
    Wrapper class for sequence losses.
    Rearranges outputs and labels for 
    use with standard Pytorch losses.
    """

    def __init__(self, loss: nn.Module):
        super().__init__()
        self.loss = loss

    def forward(self, outputs: torch.Tensor, labels: torch.Tensor) -> torch.Tensor:
        # outputs are expected to be of shape [B, S, C], reshape to [B * S, C]
        outputs = einops.rearrange(outputs, "b s c -> (b s) c")
        # labels are expected to be of shape [B, S], reshape to [B * S]
        labels = einops.rearrange(labels, "b s -> (b s)")
        return self.loss(outputs, labels)


class MultiLayerLoss(nn.Module):
    """
    Wrapper class for losses applied on output
    of multiple layers. Rearranges outputs and labels
    for use with standard Pytorch losses.
    """

    def __init__(self, loss: nn.Module):
        super().__init__()
        self.loss = loss

    def forward(self, outputs: torch.Tensor, labels: torch.Tensor) -> torch.Tensor:
        # outputs are expected to be of shape [L, B, ...], reshape to [L * B, ...]
        shape = outputs.shape[2:]
        outputs = outputs.view(-1, *shape)
        # labels are expected to be of shape [L, B, ...], reshape to [L * B, ...]
        shape = labels.shape[2:]
        labels = labels.view(-1, *shape)
        return self.loss(outputs, labels)


def loss_from_config(
    cfg: Dict[str, Any],
    additional_loss_fn: Optional[Callable[
        [Dict[str, Any]],
        nn.Module
    ]] = None
) -> nn.Module:
    cfg = copy.deepcopy(cfg)
    loss_type = cfg.pop("type")
    if loss_type == "cross_entropy":
        weight = cfg.get("weights", None)
        weight = torch.tensor(weight, dtype=torch.float) if weight is not None else None
        loss = nn.CrossEntropyLoss(ignore_index=cfg.get("ignore_index", -1), weight=weight)
        return loss

    elif loss_type == "binary_cross_entropy":
        weight = cfg.get("weight", None)
        weight = torch.tensor(weight, dtype=torch.float) if weight is not None else None
        loss = nn.BCELoss(weight=weight)
        return loss

    elif loss_type == "focal":
        weight = cfg.get("weight", None)
        loss = FocalLoss(
            alpha=weight,
            gamma=cfg.get("gamma", 2.),
            ignore_index=cfg.get("ignore_index", -1),
        )
        return loss

    elif loss_type == "sequence":
        loss = loss_from_config(cfg["loss"], additional_loss_fn)
        return SeqLoss(loss)

    elif loss_type == "multi_layer":
        loss = loss_from_config(cfg["loss"], additional_loss_fn)
        return MultiLayerLoss(loss)

    else:
        if additional_loss_fn is not None:
            return additional_loss_fn(cfg)
        raise ValueError(f"unknown loss type {loss_type}")

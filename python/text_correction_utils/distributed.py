import torch


class DistributedInfo:
    def __init__(
        self,
        rank: int,
        local_rank: int,
        world_size: int,
        local_world_size: int
    ) -> None:
        self.rank = rank
        self.local_rank = local_rank
        self.world_size = world_size
        self.local_world_size = local_world_size
        if torch.cuda.device_count() == local_world_size:
            device_index = self.local_rank
        elif torch.cuda.device_count() == 1:
            device_index = 0
        else:
            raise RuntimeError(
                f"expected either {local_world_size} or 1 GPUs available, "
                f"but got {torch.cuda.device_count()} GPUs instead"
            )
        self.device = torch.device(device_index)

    @property
    def is_local_main_process(self) -> bool:
        return self.local_rank == 0

    @property
    def is_main_process(self) -> bool:
        return self.rank == 0

    def __repr__(self) -> str:
        return f"DistributedDevice(rank={self.rank}, local_rank={self.local_rank}, " \
            f"world_size={self.world_size}, local_world_size={self.local_world_size}, " \
            f"device={self.device})"

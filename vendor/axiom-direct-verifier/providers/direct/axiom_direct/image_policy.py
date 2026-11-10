"""Fixed image adapter policy. The native client supplies only a model-derived flag."""

UNCENSORED_LORA_FILE = "loras/uncensored.safetensors"
UNCENSORED_LORA_BYTES = 79_744_352
UNCENSORED_LORA_SHA256 = "83aba822ad8ee10a1e836828f5088259d93bce7b5f3419b46dbc0258c2be46a9"
UNCENSORED_LORA_STRENGTH = 0.9


def image_policy(uncensored=False):
    if type(uncensored) is not bool:
        raise ValueError("native image model policy must be a boolean")
    return {"uncensored": uncensored, "lora": {
        "name": "uncensored.safetensors", "sha256": UNCENSORED_LORA_SHA256,
        "strength": UNCENSORED_LORA_STRENGTH,
    } if uncensored else None}

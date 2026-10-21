"""Pinned, local inference adapters for the optional grammar comparison.

Loading may download public model artifacts into cache_dir. Correction never
contacts an inference service. Only Liquid requires custom model code; both its
wrapper and encoder are inspected, pinned snapshots, loaded from local paths.
The optional Liquid reranker is deliberately disabled and never downloaded.
"""
from __future__ import annotations

from dataclasses import asdict, dataclass
import hashlib
import os
import re
from pathlib import Path
import tempfile
import time
from typing import Any, Callable
from urllib.request import urlopen


@dataclass(frozen=True)
class ModelSpec:
    name: str
    label: str
    model_id: str
    revision: str
    license: str
    artifact_bytes: int
    source_url: str
    notes: str


MODEL_SPECS = {
    "liquid": ModelSpec(
        "liquid", "Liquid LFM2.5 Encoder 350M (tagger only)",
        "LiquidAI/LFM2.5-Encoder-350M-Spellchecker",
        "4e2f724c10985e9ec90f34189c22be5d5f2e41e0", "LFM Open License v1.0",
        713449656, "https://huggingface.co/LiquidAI/LFM2.5-Encoder-350M-Spellchecker",
        "Published FP16 tagger; executed FP32 on CPU. Optional reranker excluded. "
        "Precision thresholds are an initial comparison setting, not calibrated.",
    ),
    "gector": ModelSpec(
        "gector", "GECToR-2024 (official RoBERTa-large)",
        "grammarly/GECToR-2024/gector-2024-roberta-large.th",
        "sha256:77e1c9e0d7ad5507c16509fd765283dbce5221552aa7a90c37d143f1aeaddb10", "No separate checkpoint licence stated; reference code Apache-2.0",
        1442177179, "https://github.com/grammarly/pillars-of-gec",
        "Official 2024 large checkpoint, not the older unofficial base checkpoint. "
        "Downloaded for local evaluation; not redistributed by this repository.",
    ),
    "coedit": ModelSpec(
        "coedit", "CoEdIT Small (community FLAN-T5)", "jbochi/coedit-small",
        "6ce9822b4ff6e4af86b70f979c890e9e41f04366", "Apache-2.0 (model card)",
        307867048, "https://huggingface.co/jbochi/coedit-small",
        "Community 77M-parameter model trained on CoEdIT, not Grammarly's 770M Large. "
        "FP32 reference weights; not the browser demo's quantized Q4 variant.",
    ),
}
LIQUID_ENCODER_ID = "LiquidAI/LFM2.5-Encoder-350M"
LIQUID_ENCODER_REVISION = "b886781f7c6f10ca9b7096e21b83e30a073c2f39"
ROBERTA_ID = "FacebookAI/roberta-large"
ROBERTA_REVISION = "722cf37b1afa9454edce342e7895e588b6ff1d59"
GECTOR_VOCAB_REVISION = "fea1532608bf50d086c0b62cb9c4fdc3e5c71daa"
GECTOR_WEIGHTS_URL = (
    "https://grammarly-nlp-data-public.s3.amazonaws.com/"
    "GECToR-2024/gector-2024-roberta-large.th"
)


class InputTooLongError(ValueError):
    """The full input cannot be evaluated; it has not been silently shortened."""


class OutputTooLongError(ValueError):
    """Generation reached its limit without completing a correction."""


def require_token_limit(count: int, limit: int, name: str) -> None:
    if count > limit:
        raise InputTooLongError(
            f"{name}: input has {count} model tokens; limit is {limit}. "
            "Split this case explicitly; no text was truncated."
        )


def normalize_word_apostrophes(text: str) -> str:
    """Normalize only word-internal curly apostrophes for Moses tokenization."""
    return re.sub(r"(?<=\w)[’ʼ](?=\w)", "'", text)


def restore_word_apostrophes(corrected: str, original: str) -> str:
    """Preserve source apostrophe glyphs when that original word still exists."""
    for match in re.finditer(r"\w+[’ʼ]\w+", original):
        word = match.group()
        canonical = normalize_word_apostrophes(word)
        corrected = re.sub(r"(?<!\w)" + re.escape(canonical) + r"(?!\w)",
                           lambda _: word, corrected)
    return corrected


def refine_checked(
    prepared: str, count_tokens: Callable[[str], int], correct_once: Callable[[str], str],
    *, limit: int, max_iter: int, name: str,
) -> str:
    """Bounded refinement that checks every full input before a model can slice it."""
    seen = {prepared}
    result = prepared
    for _ in range(max_iter):
        require_token_limit(count_tokens(prepared), limit, name)
        result = correct_once(prepared)
        if result == prepared or result in seen:
            break
        seen.add(result)
        prepared = result
    return result


def _sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _download(url: str, path: Path, expected_sha256: str | None = None) -> Path:
    """Publish a complete artifact atomically, rejecting altered official weights."""
    if path.exists():
        if expected_sha256 is None or _sha256(path) == expected_sha256:
            return path
        raise ValueError(f"Cached artifact checksum mismatch: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
            temporary = Path(output.name)
            with urlopen(url, timeout=120) as source:
                while block := source.read(8 * 1024 * 1024):
                    output.write(block)
        if expected_sha256 is not None and _sha256(temporary) != expected_sha256:
            raise ValueError(f"Downloaded artifact checksum mismatch: {url}")
        temporary.replace(path)
        return path
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def _snapshot(cache_dir: Path, repo_id: str, revision: str, patterns: list[str]) -> Path:
    from huggingface_hub import snapshot_download

    return Path(snapshot_download(
        repo_id=repo_id, revision=revision, cache_dir=cache_dir / "hub",
        allow_patterns=patterns,
    ))


class _Adapter:
    def __init__(self, spec: ModelSpec, threads: int):
        from sacremoses import MosesDetokenizer, MosesTokenizer
        self.spec = spec
        self.moses = MosesTokenizer(lang="en")
        self.detokenizer = MosesDetokenizer(lang="en")
        self.metadata: dict[str, Any] = {
            **asdict(spec), "device": "cpu", "threads": threads,
            "inference_dtype": "float32", "batch_size": 1,
        }

    def _tokens(self, text: str) -> list[str]:
        # Moses handles ASCII possessives but splits curly word apostrophes into
        # three words. Normalize those only; preserve the source glyph below.
        prepared = normalize_word_apostrophes(text)
        return self.moses.tokenize(prepared, escape=False)

    def _untokenize(self, corrected: str, original: str) -> str:
        # Do not create a diagnostic just because the tagger normalizes spacing.
        if corrected.split() == self._tokens(original):
            return original
        result = self.detokenizer.detokenize(corrected.split(), unescape=False)
        return restore_word_apostrophes(result, original)


class _Coedit(_Adapter):
    def __init__(self, cache_dir: Path, threads: int):
        import torch
        from transformers import AutoModelForSeq2SeqLM, AutoTokenizer
        super().__init__(MODEL_SPECS["coedit"], threads)
        path = _snapshot(cache_dir, self.spec.model_id, self.spec.revision, [
            "config.json", "generation_config.json", "model.safetensors",
            "tokenizer*", "special_tokens_map.json",
        ])
        self.tokenizer = AutoTokenizer.from_pretrained(path, local_files_only=True)
        self.model = AutoModelForSeq2SeqLM.from_pretrained(
            path, local_files_only=True, use_safetensors=True, dtype=torch.float32,
        ).eval()
        self.metadata.update({
            "artifact_weight_dtype": "float32", "artifact_bytes": (path / "model.safetensors").stat().st_size,
            "input_limit_tokens": 512, "max_new_tokens": 512,
            "prompt": "Fix the grammar: ", "do_sample": False, "num_beams": 1,
            "extra_artifacts": ["pinned tokenizer, model config, generation config"],
        })

    def correct(self, text: str) -> str:
        import torch
        if not text.strip():
            return text
        batch = self.tokenizer(self.metadata["prompt"] + text, return_tensors="pt", truncation=False)
        require_token_limit(batch.input_ids.shape[1], 512, "CoEdIT Small")
        with torch.inference_mode():
            output = self.model.generate(
                **batch, max_new_tokens=512, do_sample=False, num_beams=1,
            )[0]
        if self.tokenizer.eos_token_id not in output.tolist():
            raise OutputTooLongError("CoEdIT Small reached its generation limit without EOS")
        return self.tokenizer.decode(output, skip_special_tokens=True).strip()


class _Liquid(_Adapter):
    def __init__(self, cache_dir: Path, threads: int):
        import torch
        from transformers import AutoConfig, AutoModel, AutoTokenizer
        super().__init__(MODEL_SPECS["liquid"], threads)
        path = _snapshot(cache_dir, self.spec.model_id, self.spec.revision, [
            "config.json", "model.safetensors", "*.py", "tokenizer*",
        ])
        encoder = _snapshot(cache_dir, LIQUID_ENCODER_ID, LIQUID_ENCODER_REVISION, [
            "config.json", "modeling_lfm2_bidirectional.py",
        ])
        # The wrapper otherwise resolves encoder_name from moving Hub main.
        # Override that name with the local pinned config/code, without fetching
        # an unused second set of encoder weights or changing the checkpoint.
        config = AutoConfig.from_pretrained(path, trust_remote_code=True, local_files_only=True)
        config.encoder_name = str(encoder)
        self.tokenizer = AutoTokenizer.from_pretrained(
            path, local_files_only=True, trust_remote_code=True,
        )
        self.model = AutoModel.from_pretrained(
            path, config=config, trust_remote_code=True, local_files_only=True,
            use_safetensors=True, dtype=torch.float32,
        ).float().eval()
        self.metadata.update({
            "artifact_weight_dtype": "float16", "artifact_bytes": (path / "model.safetensors").stat().st_size,
            "input_limit_tokens": 128, "max_iter": 3, "keep_confidence": 0.0,
            "min_error_prob": 0.5, "rerank": False,
            "refinement_input_policy": "Full token count checked before each of up to 3 one-pass corrections; stop unchanged/cycles",
            "tokenization": "Moses English, escape=False; word apostrophes normalized and restored",
            "extra_artifacts": [{"model_id": LIQUID_ENCODER_ID, "revision": LIQUID_ENCODER_REVISION,
                                 "files": ["config.json", "modeling_lfm2_bidirectional.py"]},
                                "pinned tagger config/tokenizer/modeling_gectagger.py/reranker_gectagger.py"],
        })

    def correct(self, text: str) -> str:
        import torch
        if not text.strip():
            return text
        prepared = " ".join(self._tokens(text))
        def once(current: str) -> str:
            with torch.inference_mode():
                return self.model.correct(
                    current, tokenizer=self.tokenizer, max_iter=1, max_len=128,
                    batch_size=1, keep_confidence=0.0, min_error_prob=0.5, rerank=False,
                )

        result = refine_checked(
            prepared, lambda value: len(self.tokenizer.encode(value, add_special_tokens=False)) + 1,
            once, limit=128, max_iter=3, name="Liquid tagger",
        )
        return self._untokenize(result, text)


class _Gector(_Adapter):
    def __init__(self, cache_dir: Path, threads: int):
        import torch
        from gector.predict import edit_src_by_tags, load_verb_dict
        from transformers import AutoConfig, AutoTokenizer, RobertaModel
        super().__init__(MODEL_SPECS["gector"], threads)
        checkpoint = _download(GECTOR_WEIGHTS_URL, cache_dir / "gector" / "gector-2024-roberta-large.th",
                               self.spec.revision.removeprefix("sha256:"))
        data_dir = cache_dir / "gector" / GECTOR_VOCAB_REVISION
        base = f"https://raw.githubusercontent.com/grammarly/gector/{GECTOR_VOCAB_REVISION}/data/"
        labels_path = _download(base + "output_vocabulary/labels.txt", data_dir / "labels.txt")
        verb_path = _download(base + "verb-form-vocab.txt", data_dir / "verb-form-vocab.txt")
        self.labels = labels_path.read_text().splitlines()
        self.encode, self.decode = load_verb_dict(str(verb_path))
        self.edit_src_by_tags = edit_src_by_tags
        backbone = _snapshot(cache_dir, ROBERTA_ID, ROBERTA_REVISION, [
            "config.json", "tokenizer*", "vocab.json", "merges.txt",
        ])
        self.tokenizer = AutoTokenizer.from_pretrained(
            backbone, local_files_only=True, add_prefix_space=True,
        )
        self.tokenizer.add_special_tokens({"additional_special_tokens": ["$START"]})
        # Only tensors may be unpickled. No original AllenNLP/Python objects.
        state = torch.load(checkpoint, map_location="cpu", weights_only=True)
        prefix = "text_field_embedder.token_embedder_bert.bert_model."
        encoder_state = {k.removeprefix(prefix): v for k, v in state.items()
                         if k.startswith(prefix) and "pooler" not in k and "position_ids" not in k}
        config = AutoConfig.from_pretrained(backbone, local_files_only=True)
        config.vocab_size = encoder_state["embeddings.word_embeddings.weight"].shape[0]
        self.model = RobertaModel(config, add_pooling_layer=False).eval()
        self.model.load_state_dict(encoder_state, strict=True)
        self.label_weight = state["tag_labels_projection_layer._module.weight"]
        self.label_bias = state["tag_labels_projection_layer._module.bias"]
        self.detect_weight = state["tag_detect_projection_layer._module.weight"]
        self.detect_bias = state["tag_detect_projection_layer._module.bias"]
        if len(self.labels) != self.label_weight.shape[0]:
            raise ValueError("Official checkpoint and pinned GECToR label vocabulary do not match")
        self.metadata.update({
            "artifact_weight_dtype": "float32", "artifact_bytes": checkpoint.stat().st_size,
            "input_limit_tokens": 512, "max_iter": 3, "keep_confidence": 0.1,
            "min_error_prob": 0.65, "tokenization": "Moses English with word-apostrophe normalization; RoBERTa first-subword labels, $START anchor",
            "operating_point_source": "https://aclanthology.org/2024.bea-1.3/ (Table 7: confidence 0.1, min error probability 0.65)",
            "token_packing_reference": "https://github.com/grammarly/gector/blob/3d41d2841512d2690cffce1b5ac6795fe9a0a5dd/gector/tokenization.py",
            "packing": "No BOS/EOS; $START=50265; first subword per whitespace token. Full words retained (no upstream per-word truncation).",
            "reference_validation": {
                "date": "2026-10-20",
                "checks": "Exact token IDs and first-piece offsets vs official 3d41d284 tokenization.py; final corrections vs gector==1.2.0 sharing identical checkpoint tensors, confidence0.1/min_error0.65/3passes",
                "inputs": ["These estimates holds for every bounded operator .",
                           "She has went to the school yesterday .",
                           "A ten - year - old boy go to school .",
                           "The nonautonomous semigroup is well-posed ."],
                "result": "All 4 token-ID/offset checks and all 4 final corrections matched",
                "scope": "Short test cases, not a reproduction of the full published benchmark; current tokenizer API shim replaced removed batch_encode_plus with equivalent __call__",
            },
            "extra_artifacts": [{"model_id": ROBERTA_ID, "revision": ROBERTA_REVISION,
                                 "files": ["config.json", "tokenizer.json", "vocab.json", "merges.txt"]},
                                {"vocabulary_repository": "grammarly/gector", "revision": GECTOR_VOCAB_REVISION,
                                 "files": ["data/output_vocabulary/labels.txt", "data/verb-form-vocab.txt"]}],
            "decoder": "gector==1.2.0 edit_src_by_tags and verb dictionary; official tensor names loaded strictly",
        })

    def correct(self, text: str) -> str:
        import torch
        import torch.nn.functional as functional
        if not text.strip():
            return text
        words = ["$START", *self._tokens(text)]
        seen: set[tuple[str, ...]] = set()
        for _ in range(3):
            key = tuple(words)
            if key in seen:
                break
            seen.add(key)
            batch = self.tokenizer(words, is_split_into_words=True, add_special_tokens=False,
                                   return_tensors="pt", truncation=False)
            require_token_limit(batch.input_ids.shape[1], 512, "GECToR-2024")
            word_ids = batch.word_ids()
            starts = [i for i, word in enumerate(word_ids)
                      if word is not None and (i == 0 or word != word_ids[i - 1])]
            with torch.inference_mode():
                hidden = self.model(**batch).last_hidden_state[0, starts]
                probabilities = functional.linear(hidden, self.label_weight, self.label_bias).softmax(-1)
                detection = functional.linear(hidden, self.detect_weight, self.detect_bias).softmax(-1)
                probabilities[:, self.labels.index("$KEEP")] += 0.1
                confidence, predicted = probabilities.max(-1)
            if detection[:, 1].max().item() < 0.65:
                break
            labels = [self.labels[idx] if score >= 0.65 else "$KEEP"
                      for idx, score in zip(predicted.tolist(), confidence.tolist())]
            if all(label in ("$KEEP", "@@UNKNOWN@@", "@@PADDING@@") for label in labels):
                break
            updated = self.edit_src_by_tags([words], [labels], self.encode, self.decode)[0]
            if updated == words:
                break
            words = updated
        if words and words[0] == "$START":
            words = words[1:]
        return self._untokenize(" ".join(words), text)


def load_model(name: str, cache_dir: Path, threads: int = 4) -> _Adapter:
    """Load one model on CPU; callers should isolate models in separate processes."""
    if name not in MODEL_SPECS:
        raise ValueError(f"Unknown model {name!r}; choose {', '.join(MODEL_SPECS)}")
    if threads < 1:
        raise ValueError("threads must be at least 1")
    # Avoid tokenizer background thread pools competing with measured inference.
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    import torch
    torch.set_num_threads(threads)
    torch.manual_seed(0)
    cache_dir = Path(cache_dir)
    spec = MODEL_SPECS[name]
    weights = (cache_dir / "gector" / "gector-2024-roberta-large.th" if name == "gector" else
               cache_dir / "hub" / ("models--" + spec.model_id.replace("/", "--")) /
               "snapshots" / spec.revision / "model.safetensors")
    weights_cached = weights.is_file()
    started = time.perf_counter()
    adapter = {"liquid": _Liquid, "gector": _Gector, "coedit": _Coedit}[name](Path(cache_dir), threads)
    adapter.metadata["load_ms"] = (time.perf_counter() - started) * 1000
    adapter.metadata["weights_cached_before_load"] = weights_cached
    adapter.metadata["runtime_versions"] = {
        "torch": torch.__version__,
        "transformers": __import__("transformers").__version__,
        "tokenizers": __import__("tokenizers").__version__,
        "huggingface_hub": __import__("huggingface_hub").__version__,
    }
    return adapter

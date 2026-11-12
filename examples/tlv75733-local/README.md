# TLV75733PDBVR on a local model (draft part)

The first datasheet-assistant run on a local model, 5 November 2026: `qwen3.8:27b` (Q4_K_M) through Ollama's OpenAI-compatible endpoint on an M5 Max, with 262 144 tokens of context, vision declared and no native PDF. It read the text of Texas Instruments' TLV757P datasheet (SBVS322C, March 2024; 41 pages, not included here) and viewed pages 34 and 35 as images.

- `tlv757.recording.jsonl`: the 29 replies in the neutral recording format, replayed offline by the `pcbcraft-assist` tests (`tests/local_run.rs`).
- `tlv75733.review.md`: the review it produced.

Replay it without the model (any PDF stands in, since only the replies are replayed; the page views then answer with errors, which the recorded run never saw):

```sh
sparksmith design-part --replay examples/tlv75733-local/tlv757.recording.jsonl \
  --provider openai --base-url http://localhost:11434/v1 --model qwen3.8:27b \
  --context-tokens 262144 --vision yes --datasheet tlv757p.pdf \
  --name TLV75733PDBVR --out new.pcbcraft
```

[docs/datasheet-assistant.md](../../docs/datasheet-assistant.md#local-model-run-tlv75733pdbvr) compares it with the Opus 5.5 run on the same part.

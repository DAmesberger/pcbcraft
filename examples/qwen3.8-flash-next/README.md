# Qwen3.8 Flash Next with its tuned run profile (draft parts)

5 November 2026: `RadixArk/Qwen3.8-Flash-Next-NVFP4` through vLLM's OpenAI-compatible endpoint on a private network, 262 144 tokens of context, vision declared, no native PDF, with the `qwen3.8-flash-next` [run profile](../../docs/datasheet-assistant.md#run-profiles) in `assist-profiles/` (a user profile: copy it into `<config>/assist-profiles/` to use it; nothing model-specific is built in). Two parts, two Texas Instruments datasheets (not included here), read as text with pages viewed as images:

- TLV75733PDBVR, a 3.3 V LDO in SOT-23-5 (DBV), from the TLV757P datasheet (SBVS322C): `tlv757.recording.jsonl` (16 replies) and `tlv75733.review.md`.
- TPD2E009DBZR, a two-channel ESD diode in SOT-23-3 (DBZ), from the TPD2E009 datasheet (SLVS953B): `tpd2e009.recording.jsonl` (11 replies) and `tpd2e009.review.md`.

The recordings hold the model's replies only. The `pcbcraft-assist` tests (`tests/flash_next.rs`) replay them offline and check the pinout, the tabbed package form, every pad against TI's land pattern (DBV0005A: 0.6 × 1.1 mm, rows 2.6 mm apart; DBZ0003A: 0.6 × 1.3 mm, rows 2.1 mm apart), the 0.95 mm pitch and the report accepted on the first try.

Replay one without the model (any PDF stands in; the recorded page views then answer with errors):

```sh
pcbcraft-cli design-part --replay examples/qwen3.8-flash-next/tlv757.recording.jsonl \
  --provider openai --base-url http://localhost:8000/v1 --model RadixArk/Qwen3.8-Flash-Next-NVFP4 \
  --context-tokens 262144 --vision yes --datasheet tlv757p.pdf \
  --name TLV75733PDBVR --out new.pcbcraft
```

[docs/datasheet-assistant.md](../../docs/datasheet-assistant.md#tuning-qwen38-flash-next) has the tuning steps and the comparison with Opus 5.5.

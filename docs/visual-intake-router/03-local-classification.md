# Step 3 — Local classification and routing evidence

## Goal

Produce useful, explainable routing evidence without sending image pixels or
recognized text to an external service.

The classifier is a host edge effect. Its recorded output is the fact; replay
never reruns classifiers.

Apple Vision references:

- [VNClassifyImageRequest](https://developer.apple.com/documentation/vision/vnclassifyimagerequest)
- [VNRecognizeTextRequest](https://developer.apple.com/documentation/vision/vnrecognizetextrequest)

## Pipeline

Run bounded stages over the intake preview:

1. **Metadata:** dimensions, aspect ratio, Photos media subtype, source scope,
   and screenshot flag.
2. **Image classification:** Apple Vision image labels and confidences.
3. **Text detection/OCR:** only when text-like regions are present.
4. **Document cues:** document rectangle, text density, currency/date/totals
   patterns, and table/line structure.
5. **Intent mapper:** deterministic mapping from bounded evidence to
   `visual-intent-v1`.
6. **Sensitivity screen:** document/person/private-text indicators that limit
   automatic routing.

Do not make a local large-language model mandatory for v1. A later local model
may improve intent mapping behind a versioned classifier id.

## Recorded result

```json
{
  "schema": 1,
  "classifier": {
    "id": "macos-vision-router",
    "version": "1",
    "osRevision": "recorded-bounded-value"
  },
  "intents": [
    {
      "name": "food",
      "confidence": 0.94,
      "evidenceCodes": ["vision.food", "low_text_density"]
    }
  ],
  "contentFlags": [
    "photo"
  ],
  "sensitivity": {
    "level": "normal",
    "reasonCodes": []
  },
  "ocr": {
    "performed": false,
    "textHash": null,
    "lineCount": 0,
    "currencySignals": []
  }
}
```

The event records evidence codes, counts, classifier identity, and a hash of
recognized text. It does not record OCR body text. A destination that requires
OCR runs or requests its own processing after receiving the image.

## Intent mapping

Initial deterministic examples:

| Evidence | Intent contribution |
| --- | --- |
| Food labels, plate/table cues, low document score | `food` |
| High text density, currency symbol, subtotal/total/date cues | `receipt` |
| Structured supplier/customer fields, invoice-number cue, amount due | `invoice` |
| Document rectangle and high text density without financial cues | `document` |
| Large handwriting-like regions or board rectangle | `whiteboard` |
| Photos screenshot subtype | `screenshot` |
| Product/packaging labels plus barcode cue | `product` |

String matching uses locale-aware, bounded dictionaries shipped with the host.
User OCR text does not enter logs, telemetry, or classifier descriptions.

## Confidence

The intent mapper outputs calibrated scores only after a fixture corpus exists.
Before calibration, label scores `experimental` and never auto-route from them.

Calibration set:

- at least 30 food images;
- 30 receipts across multiple layouts and currencies;
- 20 invoices;
- 20 general documents;
- 20 screenshots;
- 20 whiteboards/handwritten notes;
- 30 unrelated images.

Fixtures must be synthetic, licensed, or user-provided explicitly for this test.
Do not copy personal Photos assets into the repository.

Measure:

- top-1 intent accuracy;
- top-3 recall;
- false automatic-route rate;
- sensitivity false-negative rate;
- classification latency and peak memory.

Automatic routing is disabled until the false automatic-route rate satisfies
the rollout gate in [06-implementation-and-qa.md](06-implementation-and-qa.md).

## Sensitivity

V1 sensitivity levels:

- `normal`
- `review-required`
- `blocked-from-automatic-external-processing`

Review-required evidence includes:

- person/face present;
- dense document text;
- financial-document cues;
- identity-document cues;
- medical-document cues;
- handwritten private note cues.

This is a routing safeguard, not a claim that Terrane can reliably identify all
sensitive content. The UI must say that automatic detection can be wrong.

## Versioning

Every result records:

- classifier id and version;
- intent taxonomy version;
- routing-policy version;
- bounded OS/Vision revision;
- source preview hash.

Updating a classifier does not silently rewrite old classifications. Optional
reclassification is an explicit user or migration action that records a new
`intake.image-classified` version.

## Performance budgets

- Preview maximum edge: 768 px.
- Classifier wall time target: p95 under 1 second on supported Apple Silicon.
- OCR runs only when text detection passes a threshold.
- Maximum OCR lines inspected: 100.
- Maximum retained Vision labels: 20.
- At most two items classify concurrently by default.
- Thermal pressure pauses catch-up classification before it affects foreground
  use.

## Tests

- Pure intent-mapper unit tests with fixed evidence JSON.
- Vision adapter tests with synthetic/licensed image fixtures.
- OCR redaction test proving recognized body text never enters events or logs.
- Classifier-version replay test.
- Sensitivity policy tests that prevent automatic external analysis.
- Performance receipt for the fixture corpus on the supported Mac baseline.
- Visual Inbox explanation test showing intent, confidence, and evidence codes
  in user language rather than raw Vision labels.

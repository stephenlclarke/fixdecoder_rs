# Appendix D Samples

This directory contains a generated FIX sample corpus for the FIX Protocol
Appendix D / "Order State Changes" scenarios.

Source material:

- HTML: <https://www.fixtrading.org/online-specification/order-state-changes/>
- PDF: <https://www.fixtrading.org/wp-content/uploads/download-manager-files/FIX-Latest-as-of-EP284-Order-State-Changes.pdf>

What is generated:

- `general/*.fix`: one valid FIX stream for each general scenario
- `exchange/*.fix`: one valid FIX stream for each exchange-specific scenario
- `main` variants: the primary non-italic path from the matrix
- `altNN` variants: short valid branches for italic alternative rows
- `all.fixlog`: aggregate mixed logfile containing the whole corpus
- `manifest.json`: index of every generated file

The messages are synthetic but standards-consistent:

- `BodyLength (9)` and `CheckSum (10)` are recalculated
- message shapes come from the official matrices
- FIX 4.4 field definitions and enum values come from `resources/FIX44.xml`

Regenerate the corpus with:

```bash
make appendix-d-samples
```

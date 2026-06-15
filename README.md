![repo logo](docs/repo-logo.png)
![repo title](docs/repo-title.png)

---

[![Quality Gate Status](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=alert_status)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Bugs](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=bugs)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Code Smells](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=code_smells)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Coverage](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=coverage)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Duplicated Lines (%)](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=duplicated_lines_density)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Lines of Code](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=ncloc)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Reliability Rating](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=reliability_rating)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Security Rating](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=security_rating)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Technical Debt](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=sqale_index)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Maintainability Rating](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=sqale_rating)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
[![Vulnerabilities](https://sonarcloud.io/api/project_badges/measure?project=stephenlclarke_fixdecoder_rs&metric=vulnerabilities)](https://sonarcloud.io/summary/new_code?id=stephenlclarke_fixdecoder_rs)
![Repo Visitors](https://visitor-badge.laobi.icu/badge?page_id=stephenlclarke.fixdecoder_rs)

---

# Steve's FIX Decoder / logfile prettify utility

This is my attempt to create an "all-singing / all-dancing" utility to pretty-print logfiles containing FIX Protocol messages while simultaneously learning **Rust** (after first building an earlier version in Go) and trying to incorporate SonarQube Code Quality metrics.

I have written utilities like this in past in Java, Python, C, C++, [go](https://github.com/stephenlclarke/fixdecoder) and even in Bash/Awk!! This is my favourite one so far — and now it is fully native Rust.

![repo title](docs/example.png)

---

<p align="center">
  <a href="https://buy.stripe.com/8x23cvaHjaXzdg30Ni77O00">
    <img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-❤️-brightgreen?style=for-the-badge&logo=buymeacoffee&logoColor=white" alt="Buy Me a Coffee">
  </a>
  &nbsp;
  <a href="https://github.com/stephenlclarke/fixdecoder/discussions">
    <img src="https://img.shields.io/badge/Leave%20a%20Comment-💬-blue?style=for-the-badge" alt="Leave a Comment">
  </a>
</p>

<p align="center">
  <sub>☕ If you found this project useful, consider buying me a coffee or dropping a comment — it keeps the caffeine and ideas flowing! 😄</sub>
</p>

---

## What is it

fixdecoder is a FIX-aware “tail-like” tool and dictionary explorer. It reads from stdin or multiple log files, detects and prettifies FIX messages in stream, and fits naturally into pipelines. Each highlighted message is followed by a detailed tag breakdown using the correct dictionary for BeginString (8) or, for `FIXT.1.1` sessions, the negotiated application version from `ApplVerID`/`DefaultApplVerID` (`1128`/`1137`) carried on the session. It can validate on the fly (`--validate`), reporting protocol issues as it decodes, and track order state with summaries (`--summary`). In offline multi-file mode, independent files are processed concurrently and emitted in argv order. For lookups, `--info` shows available/overridden dictionaries, and `--message`, `--component`, or `--tag` inspect definitions in the selected FIX version (`--fix` or default) without a live decode.

## Quick start

```bash
# Stream and prettify stdin (pipeline-friendly)
cat fixlog.txt | fixdecoder

# Stream with validation + order summaries
cat fixlog.txt | fixdecoder --validate --summary
```
<!-- Screenshot: cat fixlog.txt | fixdecoder -->
<!-- Screenshot: cat fixlog.txt | fixdecoder --validate --summary -->

## Running the fixdecoder utility

You can run fixdecoder anywhere you can run a Rust binary — no extra OS dependencies or runtime services are required. It ships with a full set of embedded FIX dictionaries. The sections below cover the key options for selecting and browsing dictionaries, controlling output/formatting, and adjusting processing modes.

The decoder now also exposes bat-style presentation controls for terminal use. You can add line numbers with `--number`, switch decorations with `--style=plain|numbers|header|grid|full`, disable decoration with `--plain`, and control paging with `--paging=auto|never|always`, `--pager=<CMD>`, and `--nowrap`.

<!-- regen-readme:start --section=usage -->

## Full Usage Examples

The text below is generated from `resources/messages/usage_en.txt`, the same usage text printed after `fixdecoder --help`.

```text
Command line option examples:

  FIX dictionary lookup

    Query FIX dictionary contents by FIX Message Name or MsgType:

      fixdecoder [[--fix=44] [--xml=FILE --xml=FILE2 ...]] [--message[=NAME|MSGTYPE] [--verbose] [--column] [--header] [--trailer]]

      $ fixdecoder --message=NewOrderSingle --verbose --column --header --trailer
      $ fixdecoder --message=D --verbose --column --header --trailer

    Query FIX dictionary contents by FIX Tag number:

      fixdecoder [[--fix=44] [--xml=FILE --xml=FILE2 ...]] [--tag[=TAG] [--verbose] [--column]]

      $ fixdecoder --tag=44 --verbose --column

    Query FIX dictionary contents by FIX Component Name:

      fixdecoder [[--fix=44] [--xml=FILE --xml=FILE2 ...]] [--component[=NAME] [--verbose] [--column]]

      $ fixdecoder --component=Instrument --verbose --column

  Show summary information about available FIX dictionaries:

    fixdecoder [[--fix=44] [--xml=FILE --xml=FILE2 ...]] [--info]

    $ fixdecoder --info

  Prettify FIX log files with optional validation and obfuscation. Bat-style
  viewing controls are available via --style, --plain, --number, --paging,
  --pager, --nowrap, and --nocounts. If output is piped then colour is disabled
  by default but can be forced on with --colour=yes or --color=always.
  Shell-style default options may also be supplied through FIXDECODER_DEFAULT_ARGS:

    fixdecoder [--xml=FILE --xml=FILE2 ...] [--validate]
               [--colour=yes|no|auto] [--style=STYLE] [--plain]
               [--number] [--paging=auto|never|always] [--pager=CMD]
               [--nowrap] [--nocounts] [--secret] [--summary] [--follow]
               [--fix=VER] [--delimiter=CHAR] [file1.log file2.log ...]

    Validate and Obfuscate a FIX logfile.

    $ fixdecoder --validate --secret logs/fix.log

    Decode all the NewOrderSingle messages in a FIX logfile and output the fix
    messages using a custom delimiter also force colour mode because this example
    pipes the output into less. Normally colour mode is turned off when piping
    the output due to the output containing ANSI control chars which may mess up
    processing further down the pipe chain.

    $ grep '35=D' logs/fix.log | fixdecoder --colour=yes --delimiter='|' | less

    Suppress the final message count summary when you only want decoded
    messages:

    $ fixdecoder --nocounts logs/fix.log

    Show bat-style file headers and line numbers, but disable paging for
    follow-mode output:

    $ fixdecoder --style=header,grid --number --paging=never --follow logs/fix.log

    Enable 10-column horizontal scrolling in the pager for wide decoded
    lines. Without --nowrap, wrapped paging stays wrapped even if LESS
    requests chopped lines:

    $ fixdecoder --paging=always --nowrap logs/fix.log

    Apply default viewing options from the environment. Command-line
    values are applied afterwards and override single-value defaults.
    Keep input files on the real command line:

    $ export FIXDECODER_DEFAULT_ARGS='--style=full --paging=always --nowrap'
    $ fixdecoder logs/fix.log

    Force the decoding of a FIX log to use the FIX 4.4 dictionary. Only uses the
    version of the FIX dictionary specified in the FIX message header if the tag
    being processed is not defined in the override dictionary. For example
    FIX 4.4 does not have the FIX 4.2 tag 20 (ExecTransType)

    $ fixdecoder --fix=44 trades.log

    Process a FIX log file and display an order summary for each order that is processed.

    $ fixdecoder --summary --follow logs/fix.log

    Generate obfuscated .secret copies of mixed FIX log files. Rewritten FIX
    messages have BodyLength and CheckSum updated so they remain valid:

    $ fixdecoder --secret-files logs/fix.log
    $ fixdecoder --secret-files --secret-dir redacted logs/fix.log logs/fix2.log

    Show the full help or version details:

    $ fixdecoder --help
    $ fixdecoder --version
```

<!-- regen-readme:end --section=usage -->

## Key options at a glance

- Dictionaries: `--xml`, `--fix`, `--info`, `--message`, `--component`, `--tag`
- Output/layout: `--column`, `--verbose`, `--header`, `--trailer`, `--colour`, `--delimiter`
- Bat-style viewing: `--style`, `--plain`, `--number`, `--paging`, `--pager`, `--nowrap`
- Processing modes: `--follow`, `--validate`, `--secret`, `--secret-files`, `--summary`, `--nocounts`

### `--xml`

The `--xml` flag lets you load custom FIX dictionaries from XML files; you can pass it multiple times to register several custom dictionaries. Each file is parsed, normalised to a canonical key (e.g., FIX44, FIX50SP2), and has FIXT11 session header/trailer injected for 5.0+ if missing. Custom entries are registered for tag lookup and schema loading; they override built-ins for the same key and replace earlier `--xml` files for that key, with warnings emitted in both cases.

The XML dictionaries can be downloaded from the [QuickFIX GitHub Repo](https://github.com/quickfix/quickfix/tree/master/spec)

<!-- regen-readme:start --option=--xml -->
Example output:

```bash
$ fixdecoder --xml resources/FIX44.xml --fix=44 --info
Available FIX Dictionaries: FIX27,FIX30,FIX40,FIX41,FIX42,FIX43,FIX44,FIX50,FIX50SP1,FIX50SP2,FIXT11

Loaded dictionaries:
   Version     ServicePack   Fields  Components    Messages Source
   FIX27                 0      138           2          27 built-in alias of FIX40
   FIX30                 0      138           2          27 built-in alias of FIX40
   FIX40                 0      138           2          27 built-in
   FIX41                 0      206           2          28 built-in
   FIX42                 0      405           2          46 built-in
   FIX43                 0      635          12          68 built-in
  *FIX44                 0      912         106          93 resources/FIX44.xml
   FIX50                 0     1090         123          93 built-in
   FIX50SP1              1     1373         165         105 built-in
   FIX50SP2              2     6028         727         156 built-in
...
```
<!-- regen-readme:end --option=--xml -->

### `--fix`

The `--fix` option allows you to specify the default FIX dictionary. This defaults to FIX 4.4 (`44`). It accepts either just the version digits (e.g., `44`, `4.4`) or the same value prefixed with FIX/fix (e.g., `FIX44`, `fix4.4`). The parser normalises your input by stripping dots, uppercasing, and adding FIX if it’s missing; it then checks that key against built‑ins (`FIX27`…`FIXT11`) and any custom `--xml` overrides. If the normalised key isn’t known, it errors. `FIX27` and `FIX30` are accepted intentionally as compatibility aliases for the embedded `FIX40` dictionary; they are not separate built-in XML specs.

<!-- regen-readme:start --option=--fix -->
Example output:

```bash
$ fixdecoder --fix=FIX50SP2 --info
Available FIX Dictionaries: FIX27,FIX30,FIX40,FIX41,FIX42,FIX43,FIX44,FIX50,FIX50SP1,FIX50SP2,FIXT11

Loaded dictionaries:
   Version     ServicePack   Fields  Components    Messages Source
   FIX27                 0      138           2          27 built-in alias of FIX40
   FIX30                 0      138           2          27 built-in alias of FIX40
   FIX40                 0      138           2          27 built-in
   FIX41                 0      206           2          28 built-in
   FIX42                 0      405           2          46 built-in
   FIX43                 0      635          12          68 built-in
   FIX44                 0      912         106          93 built-in
   FIX50                 0     1090         123          93 built-in
   FIX50SP1              1     1373         165         105 built-in
  *FIX50SP2              2     6028         727         156 built-in
...
```
<!-- regen-readme:end --option=--fix -->

### `--info`

`--info` is an informational mode: it prints the list of available FIX dictionary keys (built-ins plus any loaded via `--xml`), then a table of loaded dictionaries with counts and their source (built-in vs file path). The table highlights the currently selected/default FIX version (from `--fix` or the default `44`) with a leading `*` so you can see which dictionary will be used. Alias entries such as `FIX27` and `FIX30` are shown explicitly as aliases of `FIX40`. It does not decode messages or print schema details; it’s meant to verify which dictionaries are present, which ones are being overridden by custom XML, and which version is active.

<!-- regen-readme:start --option=--info -->
Example output:

```bash
$ fixdecoder --info
Available FIX Dictionaries: FIX27,FIX30,FIX40,FIX41,FIX42,FIX43,FIX44,FIX50,FIX50SP1,FIX50SP2,FIXT11

Loaded dictionaries:
   Version     ServicePack   Fields  Components    Messages Source
   FIX27                 0      138           2          27 built-in alias of FIX40
   FIX30                 0      138           2          27 built-in alias of FIX40
   FIX40                 0      138           2          27 built-in
   FIX41                 0      206           2          28 built-in
   FIX42                 0      405           2          46 built-in
   FIX43                 0      635          12          68 built-in
  *FIX44                 0      912         106          93 built-in
   FIX50                 0     1090         123          93 built-in
   FIX50SP1              1     1373         165         105 built-in
   FIX50SP2              2     6028         727         156 built-in
...
```
<!-- regen-readme:end --option=--info -->

## Querying the FIX dictionaries `--message`, `--component` and `--tag`

Use these flags to explore the active FIX dictionary. `--verbose` adds detail / metadata, `--column` uses a compact table layout. `--header`/`--trailer` only apply to `--message` and `--component` (not `--tag`).

### `--message[=<NAME|MsgType>]`

Browse messages. With no value, list all message types (use --`column` for a compact view). The listing is grouped into `Session/Admin` and business buckets such as `Order Flow`, `Quotes & Pricing`, and `Market Data`. The `Session/Admin` split comes from the FIX dictionary metadata (`msgcat=admin`), while the business buckets use an explicit reviewed `MsgType` mapping rather than name heuristics. That mapping is generated from the official FIX Trading Community business-area pages for [Pre-Trade](https://www.fixtrading.org/online-specification/business-area-pretrade/), [Trade](https://www.fixtrading.org/online-specification/business-area-trade/), [Post-Trade](https://www.fixtrading.org/online-specification/business-area-posttrade/), and [Infrastructure](https://www.fixtrading.org/online-specification/business-area-infrastructure/). With a name or MsgType (e.g., `D` or `NewOrderSingle`), render the message structure (fields, components, repeating groups); `--header`/`--trailer` include session blocks. Reports “Message not found” if absent.

<!-- regen-readme:start --option=--message -->
Example output:

```bash
$ fixdecoder --fix=44 --message=D --column
Message: NewOrderSingle (D)
    Message: Body
          11: ClOrdID (STRING) - (Y)
         526: SecondaryClOrdID (STRING)
         583: ClOrdLinkID (STRING)
   Component: Parties
         453: NoPartyIDs (NUMINGROUP)
               448: PartyID (STRING)
               447: PartyIDSource (CHAR)
               452: PartyRole (INT)
         Component: PtysSubGrp
               802: NoPartySubIDs (NUMINGROUP)
                     523: PartySubID (STRING)
                     803: PartySubIDType (INT)
         229: TradeOriginationDate (LOCALMKTDATE)
          75: TradeDate (LOCALMKTDATE)
           1: Account (STRING)
         660: AcctIDSource (INT)
         581: AccountType (INT)
         589: DayBookingInst (CHAR)
         590: BookingUnit (CHAR)
         591: PreallocMethod (CHAR)
          70: AllocID (STRING)
   Component: PreAllocGrp
          78: NoAllocs (NUMINGROUP)
                79: AllocAccount (STRING)
...
```
<!-- regen-readme:end --option=--message -->

### `--component[=<NAME>]`

Browse components. With no value, list all components (or use `--column`). With a name, render that component’s fields, nested components, and repeating groups. Reports “Component not found” if absent.

<!-- regen-readme:start --option=--component -->
Example output:

```bash
$ fixdecoder --fix=44 --component=Instrument --column
Component: Instrument
      55: Symbol (STRING)
      65: SymbolSfx (STRING)
      48: SecurityID (STRING)
      22: SecurityIDSource (STRING)
Component: SecAltIDGrp
     454: NoSecurityAltID (NUMINGROUP)
           455: SecurityAltID (STRING)
           456: SecurityAltIDSource (STRING)
     460: Product (INT)
     461: CFICode (STRING)
     167: SecurityType (STRING)
     762: SecuritySubType (STRING)
     200: MaturityMonthYear (MONTHYEAR)
     541: MaturityDate (LOCALMKTDATE)
     201: PutOrCall (INT)
     224: CouponPaymentDate (LOCALMKTDATE)
     225: IssueDate (LOCALMKTDATE)
     239: RepoCollateralSecurityType (STRING)
     226: RepurchaseTerm (INT)
     227: RepurchaseRate (PERCENTAGE)
     228: Factor (FLOAT)
...
```
<!-- regen-readme:end --option=--component -->

### `--tag[=<NUMBER>]`

Browse fields. With no value, list all tags (or use `--column`). With a tag number, show that field’s details (name, type, enums, etc.). Reports “Tag not found” if absent.

<!-- regen-readme:start --option=--tag -->
Example output:

```bash
$ fixdecoder --fix=44 --tag=44 --verbose --column
  44: Price (PRICE)
```
<!-- regen-readme:end --option=--tag -->

### `--validate`

Validate each decoded FIX message against the active dictionary (honours `--fix` and any `--xml` overrides). Checks MsgType, BodyLength, checksum, required fields, enum/type correctness, field ordering, repeating-group structure, and duplicate disallowed tags. In validation mode, clean messages are suppressed and only invalid messages are rendered with inline/matching error annotations. It doesn’t stop the stream, so it’s useful for scanning large logs for protocol issues without flooding the output with clean traffic.

<!-- regen-readme:start --option=--validate -->
Example output:

```bash
$ printf '<invalid FIX>' | fixdecoder --fix=44 --validate --nocounts --colour=no
Line 1: 8=FIX.4.4|9=005|10=000|
     8 (BeginString): FIX.4.4
     9 (BodyLength): 005
    10 (CheckSum): 000  Checksum mismatch: got 000, expected 045
    35 (MsgType): Missing required tag 35 (MsgType)
```
<!-- regen-readme:end --option=--validate -->

### `--secret`

Obfuscate sensitive FIX fields while decoding. When enabled, values for a predefined set of sensitive tags (e.g., session IDs, sender/target IDs) are replaced with stable aliases (e.g., `SenderCompID0001`) so logs stay readable without exposing real identifiers. Obfuscation is applied per line/message and resets between files; disabled by default.

<!-- regen-readme:start --option=--secret -->
Example output:

```bash
$ printf '<FIX log>' | fixdecoder --fix=44 --secret --nocounts --delimiter='|' --colour=no
8=FIX.4.4|9=45|35=0|49=SenderCompID0001|56=TargetCompID0001|10=173|

     8 (BeginString): FIX.4.4
     9 (BodyLength): 45
    35 (MsgType): 0 (HEARTBEAT)
    49 (SenderCompID): SenderCompID0001
    56 (TargetCompID): TargetCompID0001
    10 (CheckSum): 173
```
<!-- regen-readme:end --option=--secret -->

### `--secret-files`

Generate obfuscated `.secret` copies of the input files and exit. This mode is meant for mixed logfiles: each detected FIX message inside a line is rewritten with stable aliases for sensitive tags, and the rewritten message has `BodyLength (9)` and `CheckSum (10)` recalculated so it remains valid FIX after redaction. By default, each output sits next to its source with `.secret` inserted before the final extension, for example `orders.log` becomes `orders.secret.log`. Existing files are never overwritten. Use `--secret-dir=<DIR>` to write all generated secret files into a separate directory instead of beside the inputs.

<!-- regen-readme:start --option=--secret-files -->
Example output:

```bash
$ fixdecoder --secret-files target/readme-examples/orders.log
Wrote secret file: target/readme-examples/orders.secret.log
```
<!-- regen-readme:end --option=--secret-files -->

### `--colour[=yes|no|auto]`

Control coloured output. By default, colours are shown when writing to a terminal and disabled when output is piped. Use `--colour`/`--colour=yes` (or `--color=always`) to force colours on, `--colour=no` (or `--color=never`) to force them off, and `--colour=auto` to return to the default terminal-sensitive behaviour.

<!-- regen-readme:start --option=--colour -->
Example output:

```bash
$ printf '<FIX log>' | fixdecoder --fix=44 --nocounts --colour=no
8=FIX.4.4|9=22|35=0|49=BUY1|56=SELL1|10=168|

     8 (BeginString): FIX.4.4
     9 (BodyLength): 22
    35 (MsgType): 0 (HEARTBEAT)
    49 (SenderCompID): BUY1
    56 (TargetCompID): SELL1
    10 (CheckSum): 168
```
<!-- regen-readme:end --option=--colour -->

### Bat-style viewing controls

fixdecoder keeps its FIX-aware decode output, but now borrows bat’s terminal ergonomics:

- `--style=plain|numbers|header|grid|full` toggles bat-style decorations around the decoded stream. `full` enables line numbers and separators together. When reading real files, the output always begins with a five-line file banner showing `Filename:` and a UTC `Last Modified:` timestamp, even in plain mode.
- `--plain` disables decorative stdin headers, line numbers, and separators, but real files still keep the five-line file banner.
- `--number` adds input line numbers to the rendered source lines.
- `--paging=auto|never|always` controls whether output is sent through a pager. `auto` uses a pager only for interactive terminals, `never` disables it, and `always` forces it for interactive terminals.
- `--pager=<CMD>` overrides the pager command. If unset, fixdecoder honours `PAGER` and otherwise falls back to `less`.
- `--nowrap` enables chopped lines and horizontal scrolling in pager mode. With the default `less` pager, left and right arrow movement shifts 10 columns at a time so wide FIX lines remain practical to inspect. Without `--nowrap`, fixdecoder keeps wrapped pager output even if inherited `less` settings request chopped lines. It has no effect when output is not being paged.
- `FIXDECODER_DEFAULT_ARGS` can hold shell-style default options such as `--style=full --paging=always --nowrap`. These defaults are applied before the real command line, so later single-value CLI options such as `--fix`, `--paging`, or `--pager` override the environment value. Keep input files on the real command line.

Examples:

```bash
# Bat-style headers + grids with line numbers
fixdecoder --style=header,grid --number orders.log

# Disable paging for follow-mode style monitoring
tail -f orders.log | fixdecoder --paging=never --number

# Use horizontal scrolling instead of wrapped lines in the pager
fixdecoder --paging=always --nowrap orders.log

# Apply your preferred viewing defaults to every run
export FIXDECODER_DEFAULT_ARGS='--style=full --paging=always --nowrap'
fixdecoder orders.log
```

### `--delimiter=<CHAR>`

Set the display delimiter between FIX fields (default: `SOH`). Specify a single character after `=` sign.

Accepted values:

- A single literal character (e.g.`,`, `|`, or a single Unicode character like `—`).
  
- SOH (case-insensitive) or a hex escape like `\x01`/`0x01` (quote to protect the backslash, e.g. `--delimiter='\x1f'`).

Empty values or anything longer than one character are rejected.

<!-- regen-readme:start --option=--delimiter -->
Example output:

```bash
$ printf '<FIX log>' | fixdecoder --fix=44 --nocounts --delimiter=' ' --colour=no
8=FIX.4.4 9=22 35=0 49=BUY1 56=SELL1 10=168

     8 (BeginString): FIX.4.4
     9 (BodyLength): 22
    35 (MsgType): 0 (HEARTBEAT)
    49 (SenderCompID): BUY1
    56 (TargetCompID): SELL1
    10 (CheckSum): 168
```
<!-- regen-readme:end --option=--delimiter -->

### `--nocounts`

Disable the final `Message Counts:` summary that is normally printed after decoded message output. This is useful when you want output that contains only the original log lines and their pretty-printed FIX tag breakdowns, for example when copying examples into documentation or feeding decoded output into another tool.

<!-- regen-readme:start --option=--nocounts -->
Example output:

```bash
$ printf '<FIX log>' | fixdecoder --fix=44 --nocounts --colour=no
8=FIX.4.4|9=22|35=0|49=BUY1|56=SELL1|10=168|

     8 (BeginString): FIX.4.4
     9 (BodyLength): 22
    35 (MsgType): 0 (HEARTBEAT)
    49 (SenderCompID): BUY1
    56 (TargetCompID): SELL1
    10 (CheckSum): 168
```
<!-- regen-readme:end --option=--nocounts -->

### `-f`, `--follow`

Stream input like `tail -f`. Keeps reading and decoding as new data arrives on stdin or a file, sleeping briefly on `EOF` rather than exiting, until interrupted. This mirrors `tail -f` behaviour but with FIX decoding, validation, and prettification applied in real time.

### `--summary`

Track FIX order lifecycles and emit a summary instead of full decoded messages. When enabled, each application message that can be tied to an order flow is consumed into an order tracker (keyed by `OrderID`/`ClOrdID`/`OrigClOrdID`), updating state, quantities, prices, and events. Standard FIX session/admin messages such as `Heartbeat`, `Logon`, `Logout`, and `SequenceReset` are ignored in this mode, and application messages without a resolvable order identifier are skipped as non-order flow traffic. Invalid order-flow messages are still shown in the timeline and raw FIX list; in the timeline only the invalid suffix is red, while the original FIX `Text` value keeps its normal colour. Message-count tables are grouped so session/admin traffic is separated from business traffic, and business messages are bucketed into families such as order flow, quotes/pricing, and market data using the same explicit `MsgType` taxonomy described in `--message`, sourced from the official FIX Trading Community business-area pages. On an interactive terminal, `--summary` now opens a built-in split pager by default; the left pane stays fixed and shows the current order block plus cumulative order/message counts from the start of the file through the bottom of the visible right-hand pane. Use `--paging=never` if you want the plain text summary written directly to stdout. At the end (or live in `--follow` mode) it prints a concise per-order summary/footer using the chosen display delimiter. This mode suppresses the usual prettified message output; use it to monitor order state across a stream or log.

<!-- regen-readme:start --option=--summary -->
Example output:

```bash
$ printf '<order FIX log>' | fixdecoder --fix=44 --summary --nocounts --paging=never --colour=no
  ORD-README-1 [New] Buy IBM
    Side     Symbol           OrderQty       Price          TradeDate    Tenor      TimeInForce        OrdType          ValueDate
    BUY      IBM              100            50.00          20260425     -          -                  LIMIT            -

  ORD-README-1 [New] Buy IBM

    Timeline:
      time                   msg                                        ExecType           OrdStatus          cum/leaves         last@price         avgPx      text
      20260425-10:00:00.000  NEW_ORDER_SINGLE [CL-README-1]             -                  -                  -/-                -@-                -          -
      20260425-10:00:01.000  EXECUTION_REPORT [CL-README-1]             New                New                0/100              0@-                0          -

Order Summary (1 open, 1 total, to fill: 1/1)
```
<!-- regen-readme:end --option=--summary -->

## Appendix D Samples

The repo now includes a generated FIX sample corpus for the FIX Protocol Appendix D / "Order State Changes" scenarios under `resources/examples/appendix_d/`. The messages are synthetic but fully valid FIX 4.4 tag-value messages with recalculated `BodyLength (9)` and `CheckSum (10)`, generated from the official FIX Trading Community [Order State Changes HTML](https://www.fixtrading.org/online-specification/order-state-changes/) and companion [PDF](https://www.fixtrading.org/wp-content/uploads/download-manager-files/FIX-Latest-as-of-EP284-Order-State-Changes.pdf). The corpus contains one main stream per scenario plus short alternate streams for the italic branch rows, a combined `all.fixlog`, and a `manifest.json` index.

## Repeating Group Samples

The repo also includes a small checked-in FIX 4.4 repeating-group corpus under `resources/examples/repeating_groups/`. These examples are synthetic but validation-clean tag-value FIX messages covering common structures such as `NoPartyIDs (453)` with nested `NoPartySubIDs (802)`, `NoAllocs (78)`, `NoOrders (73)`, and `NoMDEntries (268)`. The message layouts are based on the official FIX Trading Community [TagValue Encoding](https://www.fixtrading.org/standards/tagvalue-online/) guidance plus the corresponding FIXimate message/component pages for [Parties](https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_49484950.html?find=PartyID), [PreAllocGrp](https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_50485157.html?find=AllocQty), [AllocationInstruction](https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_495774.html?find=AllocID), and [MDFullGrp](https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_50485149.html?find=NoMDEntries). The directory contains one file per sample, a combined `all.fixlog`, and a `manifest.json` index.

# Download it

Check out the Repo's [Releases Page](https://github.com/stephenlclarke/fixdecoder_rs/releases) to see what versions are available for the computer you want to run it on.
Unix release assets are published as `.tar.gz` archives so the executable bit is preserved when you download them; extract the archive before running `fixdecoder` or `pcap2fix`. Windows releases are published as `.exe` files.
Windows executables embed the Marvin icon from `resources/icons/marvin.ico`. On macOS and Linux, the same source image is kept as `resources/icons/marvin.icns` and `resources/icons/marvin.png` for app-bundle or desktop-launcher packaging; standalone CLI binaries on those platforms do not have a portable built-in app icon format.

# Build it

Build it from source. This now requires `bash` version 5+ and a recent `Rust` toolchain (the project is tested with Rust 1.91+).
Run `make icons` if you want to regenerate the tracked icon assets from `resources/icons/marvin.png`.
Run `make message-groups` if you want to refresh the tracked `MsgType` bucket table from the official FIX Trading Community online specification pages for [Pre-Trade](https://www.fixtrading.org/online-specification/business-area-pretrade/), [Trade](https://www.fixtrading.org/online-specification/business-area-trade/), [Post-Trade](https://www.fixtrading.org/online-specification/business-area-posttrade/), and [Infrastructure](https://www.fixtrading.org/online-specification/business-area-infrastructure/).
Run `make appendix-d-samples` if you want to regenerate the tracked Appendix D sample corpus from the official FIX Trading Community order-state matrices.
Run `make repeating-group-samples` if you want to regenerate the tracked repeating-group FIX examples.

<!-- regen-readme:start --section=build-examples -->

```bash
❯ bash --version
GNU bash, version 5.3.15(1)-release (aarch64-apple-darwin25.4.0)
Copyright (C) 2025 Free Software Foundation, Inc.
License GPLv3+: GNU GPL version 3 or later <http://gnu.org/licenses/gpl.html>
```

```bash
❯ rustc --version
rustc 1.96.0 (ac68faa20 2026-05-25) (Homebrew)
```

Clone the git repo.

```bash
❯ git clone git@github.com:stephenlclarke/fixdecoder.git
Cloning into 'fixdecoder'...
...
❯ cd fixdecoder
```

Then build it. Debug version with clippy and code coverage.

If you want local Windows executables from macOS, `make build-windows` cross-compiles `fixdecoder.exe` and `pcap2fix.exe` for `x86_64-pc-windows-gnu`.

```bash
❯ make clean build scan coverage build-release
make[1]: Entering directory '.'
     Removed 7003 files, 1.1GiB total

>> Ensuring Rust toolchain and coverage tools

>> Installing llvm-tools-preview component
info: component llvm-tools is up to date

>> Ensuring FIX XML specs are present
   Compiling minimal-lexical v0.2.1
   Compiling thiserror v1.0.69
   Compiling pcap2fix v0.1.0 (pcap2fix)
   Compiling thiserror-impl v1.0.69
warning: fixdecoder@0.3.0: Building fixdecoder 0.3.0 (branch:main, commit:f4ba8ce) [rust:1.96.0]
   Compiling arrayvec v0.7.6
   Compiling circular v0.3.0
   Compiling fixdecoder v0.3.0 (.)
   Compiling etherparse v0.15.0
   Compiling nom v7.1.3
   Compiling rusticata-macros v4.1.0
   Compiling pcap-parser v0.14.1
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.94s
    Checking memchr v2.7.6
    Checking anstyle v1.0.13
    Checking libc v0.2.186
    Checking bitflags v2.10.0
    Checking regex-syntax v0.8.8
    Checking utf8parse v0.2.2
    Checking cfg-if v1.0.4
    Checking num-traits v0.2.19
    Checking is_terminal_polyfill v1.70.2
    Checking colorchoice v1.0.4
    Checking anstyle-query v1.1.5
    Checking anstyle-parse v0.2.7
    Checking strsim v0.11.1
    Checking clap_lex v0.7.6
    Checking crossbeam-utils v0.8.21
    Checking objc2-encode v4.1.0
    Checking anyhow v1.0.100
    Checking smallvec v1.15.1
    Checking anstream v0.6.21
    Checking scopeguard v1.2.0
    Checking predicates-core v1.0.9
    Checking objc2 v0.6.3
    Checking aho-corasick v1.1.4
    Checking log v0.4.29
    Checking lock_api v0.4.14
    Checking serde_core v1.0.228
    Checking clap_builder v4.5.53
    Checking crossbeam-epoch v0.9.18
    Checking float-cmp v0.10.0
   Compiling assert_cmd v2.1.1
    Checking difflib v0.4.0
    Checking core-foundation-sys v0.8.7
    Checking crossbeam-deque v0.8.6
    Checking termtree v0.5.1
    Checking normalize-line-endings v0.3.0
    Checking errno v0.3.14
    Checking parking_lot_core v0.9.12
    Checking mio v1.2.0
    Checking rayon-core v1.13.0
    Checking signal-hook-registry v1.4.8
    Checking rustix v1.1.2
    Checking parking_lot v0.12.5
    Checking rustix v0.38.44
    Checking signal-hook v0.3.18
    Checking predicates-tree v1.0.12
    Checking block2 v0.6.2
    Checking getrandom v0.3.4
    Checking nix v0.30.1
    Checking signal-hook-mio v0.2.5
    Checking wait-timeout v0.2.1
    Checking iana-time-zone v0.1.64
   Compiling fixdecoder v0.3.0 (.)
    Checking regex-automata v0.4.13
    Checking either v1.15.0
    Checking dispatch2 v0.3.0
    Checking fastrand v2.3.0
    Checking once_cell v1.21.3
    Checking rayon v1.11.0
    Checking crossterm v0.28.1
    Checking chrono v0.4.42
    Checking roxmltree v0.21.1
    Checking clap v4.5.53
    Checking ctrlc v3.5.1
    Checking shlex v1.3.0
    Checking minimal-lexical v0.2.1
   Compiling pcap2fix v0.1.0 (pcap2fix)
    Checking serde v1.0.228
    Checking nom v7.1.3
    Checking arrayvec v0.7.6
    Checking tempfile v3.23.0
    Checking terminal_size v0.4.3
    Checking circular v0.3.0
    Checking etherparse v0.15.0
    Checking thiserror v1.0.69
warning: fixdecoder@0.3.0: Building fixdecoder v0.3.0 (branch:main, commit:f4ba8ce) [rust:1.96.0]
    Checking quick-xml v0.36.2
    Checking regex v1.12.2
    Checking bstr v1.12.1
    Checking rusticata-macros v4.1.0
    Checking pcap-parser v0.14.1
    Checking predicates v3.1.3
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.55s
Running cargo-audit (text output)
      Loaded 1053 security advisories (from ~/.cargo/advisory-db)
    Scanning Cargo.lock for vulnerabilities (154 crate dependencies)
Running cargo-audit (SARIF) → target/coverage/rustsec.sarif
info: cargo-llvm-cov currently setting cfg(coverage); you can opt-out it by passing --no-cfg-coverage
   Compiling libc v0.2.186
   Compiling proc-macro2 v1.0.103
   Compiling memchr v2.7.6
   Compiling quote v1.0.42
   Compiling serde_core v1.0.228
   Compiling unicode-ident v1.0.22
   Compiling winnow v1.0.2
   Compiling autocfg v1.5.0
   Compiling version_check v0.9.5
   Compiling bitflags v2.10.0
   Compiling anstyle v1.0.13
   Compiling regex-syntax v0.8.8
   Compiling toml_parser v1.1.2+spec-1.1.0
   Compiling utf8parse v0.2.2
   Compiling num-traits v0.2.19
   Compiling aho-corasick v1.1.4
   Compiling cfg-if v1.0.4
   Compiling anstyle-parse v0.2.7
   Compiling is_terminal_polyfill v1.70.2
   Compiling colorchoice v1.0.4
   Compiling crossbeam-utils v0.8.21
   Compiling anstyle-query v1.1.5
   Compiling anstream v0.6.21
   Compiling objc2 v0.6.3
   Compiling heck v0.5.0
   Compiling strsim v0.11.1
   Compiling anyhow v1.0.100
   Compiling clap_lex v0.7.6
   Compiling regex-automata v0.4.13
   Compiling clap_builder v4.5.53
   Compiling syn v2.0.111
   Compiling errno v0.3.14
   Compiling signal-hook v0.3.18
   Compiling rustix v1.1.2
   Compiling parking_lot_core v0.9.12
   Compiling serde_spanned v1.1.1
   Compiling toml_datetime v1.1.1+spec-1.1.0
   Compiling objc2-encode v4.1.0
   Compiling toml v1.1.2+spec-1.1.0
   Compiling cfg_aliases v0.2.1
   Compiling nix v0.30.1
   Compiling crossbeam-epoch v0.9.18
   Compiling regex v1.12.2
   Compiling winresource v0.1.31
   Compiling signal-hook-registry v1.4.8
   Compiling scopeguard v1.2.0
   Compiling rayon-core v1.13.0
   Compiling predicates-core v1.0.9
   Compiling semver v1.0.27
   Compiling serde v1.0.228
   Compiling rustix v0.38.44
   Compiling log v0.4.29
   Compiling smallvec v1.15.1
   Compiling getrandom v0.3.4
   Compiling mio v1.2.0
   Compiling block2 v0.6.2
   Compiling rustc_version v0.4.1
   Compiling lock_api v0.4.14
   Compiling crossbeam-deque v0.8.6
   Compiling float-cmp v0.10.0
   Compiling normalize-line-endings v0.3.0
   Compiling assert_cmd v2.1.1
   Compiling termtree v0.5.1
   Compiling difflib v0.4.0
   Compiling clap_derive v4.5.49
   Compiling serde_derive v1.0.228
   Compiling core-foundation-sys v0.8.7
   Compiling iana-time-zone v0.1.64
   Compiling predicates v3.1.3
   Compiling predicates-tree v1.0.12
   Compiling parking_lot v0.12.5
   Compiling fixdecoder v0.3.0 (.)
   Compiling signal-hook-mio v0.2.5
   Compiling dispatch2 v0.3.0
   Compiling bstr v1.12.1
   Compiling wait-timeout v0.2.1
   Compiling fastrand v2.3.0
   Compiling either v1.15.0
   Compiling once_cell v1.21.3
   Compiling ctrlc v3.5.1
   Compiling rayon v1.11.0
   Compiling crossterm v0.28.1
   Compiling terminal_size v0.4.3
   Compiling chrono v0.4.42
   Compiling tempfile v3.23.0
   Compiling roxmltree v0.21.1
   Compiling clap v4.5.53
   Compiling shlex v1.3.0
   Compiling minimal-lexical v0.2.1
   Compiling thiserror v1.0.69
warning: fixdecoder@0.3.0: Building fixdecoder v0.3.0 (branch:main, commit:f4ba8ce) [rust:1.96.0]
   Compiling thiserror-impl v1.0.69
   Compiling pcap2fix v0.1.0 (pcap2fix)
   Compiling arrayvec v0.7.6
   Compiling nom v7.1.3
   Compiling circular v0.3.0
   Compiling etherparse v0.15.0
   Compiling rusticata-macros v4.1.0
   Compiling pcap-parser v0.14.1
   Compiling quick-xml v0.36.2
    Finished `test` profile [unoptimized + debuginfo] target(s) in 12.48s
     Running unittests src/main.rs (target/llvm-cov-target/debug/deps/fixdecoder-93217b850ffa7b10)

running 273 tests
test decoder::display::tests::layout_stats_produces_layout ... ok
test decoder::display::tests::collect_sorted_values_orders_by_enum ... ok
test decoder::display::tests::compute_values_layout_uses_max_entry ... ok
test decoder::display::tests::pad_ansi_extends_to_requested_width ... ok
test decoder::display::tests::collect_group_layout_counts_nested_components ... ok
test decoder::display::tests::print_field_renders_required_indicator ... ok
test decoder::display::tests::render_component_prints_matching_msg_type_enum_only ... ok
test decoder::display::tests::print_enum_outputs_coloured_enum ... ok
test decoder::display::tests::compute_message_layout_counts_header_and_trailer ... ok
test decoder::display::tests::print_enum_columns_respects_layout_columns ... ok
test decoder::display::tests::render_message_includes_header_and_trailer ... ok
test decoder::display::tests::tag_and_message_cells_include_expected_text ... ok
test decoder::display::tests::terminal_width_is_positive ... ok
test decoder::display::tests::render_message_keeps_group_count_tags_outside_member_indent ... ok
test decoder::display::tests::render_message_preserves_interleaved_container_order ... ok
test decoder::display::tests::visible_len_ignores_escape_sequences ... ok
test decoder::display::tests::grouped_message_listing_separates_admin_and_business_buckets ... ok
test decoder::display::tests::visible_width_counts_multibyte_glyphs_once ... ok
test decoder::display::tests::visible_width_ignores_ansi_sequences ... ok
test decoder::display::tests::visible_width_ignores_control_characters ... ok
test decoder::display::tests::write_with_padding_adds_spaces ... ok
test decoder::display::tests::cached_layout_is_reused_for_component ... ok
test decoder::message_groups::tests::explicit_mapping_matches_expected_bucket_samples ... ok
test decoder::prettifier::tests::max_visible_line_width_tracks_longest_rendered_line ... ok
test decoder::prettifier::tests::group_labels_use_group_name_without_padding ... ok
test decoder::prettifier::tests::build_tag_order_respects_annotations_and_trailer ... ok
test decoder::prettifier::tests::read_line_with_follow_returns_zero_on_eof ... ok
test decoder::prettifier::tests::header_and_trailer_are_repositioned_when_out_of_place ... ok
test decoder::prettifier::tests::render_separator_expands_when_wide_grid_is_enabled ... ok
test decoder::prettifier::tests::message_count_summary_includes_separator_before_header ... ok
test decoder::prettifier::tests::prettify_aligns_group_entries_without_header ... ok
test decoder::message_groups::tests::explicit_mapping_covers_all_embedded_application_messages ... ok
test decoder::prettifier::tests::prettify_files_preserves_input_order_when_parallelised ... ok
test decoder::prettifier::tests::source_line_visible_width_counts_line_numbers_and_soh_markers ... ok
test decoder::prettifier::tests::trim_line_endings_strips_crlf ... ok
test decoder::prettifier::tests::prettify_files_validation_skips_message_counts_for_clean_messages ... ok
test decoder::prettifier::tests::prettify_includes_missing_tag_annotations_once ... ok
test decoder::prettifier::tests::prettify_orders_without_msg_type_header_first ... ok
test decoder::prettifier::tests::render_message_counts_separates_admin_and_groups_business_messages ... ok
test decoder::schema::tests::parse_message_fields ... ok
test decoder::schema::tests::parse_message_with_components ... ok
test decoder::schema::tests::parse_simple_vec ... ok
test decoder::schema::tests::schema_tree_preserves_message_entry_order ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_a_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_d_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_A_1_d_main ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_I_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_I_1_a_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_I_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_I_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_I_1_b_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::EXCHANGE_I_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_A_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_A_1_a_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_A_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_A_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_A_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_a_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_a_alt03 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_b_alt02 ... ok
test decoder::prettifier::tests::renders_allocation_instruction_fixture_with_order_groups ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_b_alt03 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_c_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_d_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_f_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_B_1_e_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_a_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_a_alt03 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_b_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_b_alt03 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_c_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_c_alt03 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_2_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_2_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_a_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_a_alt03 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_C_3_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_1_b_alt01 ... ok
test decoder::prettifier::tests::renders_market_data_fixture_with_bid_and_offer_entries ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_1_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_1_c_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_2_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_2_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_2_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_d_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_D_2_d_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_e_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_d_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_f_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_e_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_F_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_E_1_f_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_F_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_G_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_F_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_G_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_G_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_G_1_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_a_main ... ok
test decoder::prettifier::tests::renders_parties_fixture_with_nested_party_sub_ids ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_c_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_c_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_d_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_I_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_I_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_I_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_d_alt02 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_H_1_d_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_I_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_J_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_J_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_G_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_K_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_J_1_c_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_K_1_a_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_K_1_b_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_L_1_a_alt01 ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_K_1_b_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_J_1_d_main ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_L_1_b_alt01 ... ok
test decoder::summary::tests::absorb_fields_sets_block_notice_specifics ... ok
test decoder::summary::tests::absorb_fields_sets_core_values ... ok
test decoder::summary::tests::bn_message_sets_state_and_spot_price ... ok
test decoder::summary::tests::build_summary_row_includes_bn_headers ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_J_1_a_main ... ok
test decoder::summary::tests::date_diff_days_returns_none_when_incomplete ... ok
test decoder::summary::tests::display_instrument_formats_side_and_symbol ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_L_1_a_main ... ok
test decoder::summary::tests::extract_date_part_handles_timestamp ... ok
test decoder::summary::tests::collects_states_for_single_order ... ok
test decoder::summary::tests::flow_label_skips_leading_unknown ... ok
test decoder::summary::tests::ignores_non_order_flow_messages_without_resolvable_ids ... ok
test decoder::summary::tests::ignores_standard_admin_messages ... ok
test decoder::summary::tests::preferred_settlement_date_prefers_primary_then_secondary ... ok
test decoder::summary::appendix_d_summary_tests::GENERAL_L_1_b_main ... ok
test decoder::summary::tests::render_record_header_includes_id_and_instrument ... ok
test decoder::summary::tests::resolve_key_prefers_alias_then_ids ... ok
test decoder::summary::tests::render_outputs_state_headline ... ok
test decoder::summary::tests::state_path_deduplicates_consecutive_states ... ok
test decoder::summary::tests::state_path_orders_by_time_and_deduplicates_repeated_events ... ok
test decoder::summary::tests::links_orders_using_order_id_and_cl_ord_id ... ok
test decoder::summary::tests::transact_time_falls_back_to_trade_date_for_tenor_math ... ok
test decoder::summary::tests::paged_sections_accumulate_admin_counts_through_visible_order ... ok
test decoder::summary::tests::terminal_status_from_non_exec_report_updates_header ... ok
test decoder::summary_pager::tests::cumulative_summary_tracks_bottom_of_visible_pane ... ok
test decoder::summary_pager::tests::slice_ansi_respects_visible_offsets ... ok
test decoder::summary_pager::tests::slice_ansi_offsets_ignore_control_characters ... ok
test decoder::tag_lookup::tests::detects_schema_from_default_appl_ver_id ... ok
test decoder::summary_pager::tests::section_index_tracks_scroll_position ... ok
test decoder::prettifier::tests::renders_prealloc_fixture_with_each_allocation_entry ... ok
test decoder::prettifier::tests::separators_bracket_source_line_in_wide_grid_mode ... ok
test decoder::prettifier::tests::validation_inserts_missing_tags ... ok
test decoder::validator::tests::allows_component_first_group_entries ... ok
test decoder::validator::tests::allows_repeating_group_tags ... ok
test decoder::prettifier::tests::validation_only_outputs_invalid_messages ... ok
test decoder::validator::tests::detects_body_length_mismatch ... ok
test decoder::validator::tests::detects_checksum_mismatch ... ok
test decoder::validator::tests::detects_invalid_enum_and_type ... ok
test decoder::validator::tests::detects_duplicate_top_level_tag_even_if_repeatable_elsewhere ... ok
test decoder::validator::tests::detects_invalid_numingroup_and_tag_outside_group ... ok
test decoder::validator::tests::detects_out_of_order_tags_within_group ... ok
test decoder::validator::tests::helper_checksum_and_body_length_fall_back_when_fields_are_missing ... ok
test decoder::validator::tests::detects_unknown_msg_type ... ok
test decoder::validator::tests::missing_checksum_is_reported ... ok
test decoder::validator::tests::missing_msg_type_still_reports_length_and_tag ... ok
test decoder::validator::tests::multiple_group_entries_do_not_trigger_top_level_order_errors ... ok
test decoder::validator::tests::nested_group_entries_can_be_followed_by_top_level_fields ... ok
test decoder::validator::tests::optional_group_children_are_not_required_when_group_is_absent ... ok
test decoder::validator::tests::required_group_fields_are_validated_per_entry ... ok
test decoder::validator::tests::required_group_requires_count_tag_not_child_tags ... ok
test fix::obfuscator::tests::disabled_obfuscator_returns_original_line_and_reset_is_noop ... ok
test decoder::validator::tests::helper_type_validators_cover_temporal_formats ... ok
test fix::obfuscator::tests::obfuscate_line_leaves_non_sensitive_and_malformed_fragments_unchanged ... ok
test fix::obfuscator::tests::obfuscate_line_preserves_mixed_log_context_and_repairs_fix_lengths ... ok
test fix::obfuscator::tests::split_once_accepts_equals_and_soh_delimiters ... ok
test fix::tests::choose_embedded_xml_defaults_to_fix44 ... ok
test fix::obfuscator::tests::reset_starts_aliases_over ... ok
test fix::tests::supported_versions_and_factory_cover_public_helpers ... ok
test tests::add_flag_args_sets_flags ... ok
test tests::add_entity_arg_defaults_to_true_when_missing_value ... ok
test tests::build_cli_parses_bat_style_flags ... ok
test tests::build_cli_parses_follow_and_summary_flags ... ok
test tests::build_context_disables_live_status_when_paging_is_active ... ok
test tests::component_def_has_entries_detects_fields_groups_and_components ... ok
test tests::default_less_command_adds_no_wrap_flag ... ok
test tests::build_cli_rejects_duplicate_single_value_args ... ok
test tests::dictionary_key_includes_service_pack ... ok
test tests::dictionary_marker_highlights_selected_entry ... ok
test tests::dictionary_source_prefers_custom_entry ... ok
test tests::effective_less_options_add_horizontal_scroll_only_for_nowrap ... ok
test tests::effective_less_options_strip_horizontal_scroll_when_nowrap_is_disabled ... ok
test tests::explicit_paging_overrides_summary_default ... ok
test tests::final_exit_code_marks_interrupt ... ok
test tests::ensure_session_components_backfills_missing_fix50_header_and_trailer ... ok
test tests::invalid_fix_version_errors ... ok
test tests::find_message_supports_name_and_msg_type_queries ... ok
test tests::merged_less_options_appends_once ... ok
test tests::multi_file_pager_rejects_shell_commands ... ok
test tests::multi_file_pager_requires_multiple_real_files ... ok
test tests::normalise_fix_key_handles_variants ... ok
test tests::normalise_less_command_preserves_shell_pipeline_commands ... ok
test tests::normalise_less_command_reapplies_nowrap_flags ... ok
test tests::normalise_less_command_strips_horizontal_flags_when_wrapping ... ok
test tests::pager_process_spec_appends_file_paths ... ok
test decoder::tag_lookup::tests::load_dictionary_respects_override_key ... ok
test decoder::tag_lookup::tests::new_order_single_does_not_inherit_unreachable_group_memberships ... ok
test tests::parse_colour_recognises_yes_no ... ok
test tests::parse_colour_rejects_invalid ... ok
test tests::parse_default_arg_matches_rejects_invalid_shell_quoting ... ok
test decoder::tag_lookup::tests::override_uses_fallback_dictionary_for_missing_tags ... ok
test tests::parse_default_arg_matches_rejects_input_files ... ok
test tests::parse_delimiter_accepts_hex ... ok
test decoder::tag_lookup::tests::repeatable_tags_include_nested_groups ... ok
test tests::parse_default_arg_matches_rejects_version_flag ... ok
test tests::parse_delimiter_accepts_literal ... ok
test decoder::tag_lookup::tests::session_default_guides_fixt_messages_without_appl_ver_id ... ok
test tests::parse_delimiter_rejects_empty ... ok
test decoder::tag_lookup::tests::warns_and_falls_back_on_unknown_override ... ok
test tests::parse_output_style_supports_full_and_overrides ... ok
test tests::parse_output_style_value_rejects_invalid_component ... ok
test tests::parse_paging_defaults_and_accepts_expected_values ... ok
test tests::resolve_input_files_defaults_to_stdin ... ok
test tests::parse_paging_rejects_empty_and_unknown_values ... ok
test tests::resolve_input_files_preserves_inputs ... ok
test tests::secret_output_path_inserts_suffix_before_extension ... ok
test tests::secret_output_path_uses_secret_dir_when_supplied ... ok
test tests::session_component_helpers_cover_fix50_family ... ok
test tests::valid_fix_version_passes ... ok
test tests::validate_cli_options_rejects_secret_files_without_inputs ... ok
test tests::validate_cli_options_rejects_secret_dir_without_secret_files ... ok
test tests::uses_less_pager_detects_less_by_basename ... ok
test tests::version_str_is_cached ... ok
test tests::summary_defaults_to_paging_always ... ok
test tests::version_string_matches_components ... ok
test tests::pager_writer_reports_non_zero_exit_status ... ok
test tests::summarise_dictionary_counts_header_and_trailer_once ... ok
test decoder::prettifier::tests::validation_skips_valid_messages ... ok
test decoder::prettifier::tests::wide_grid_source_separators_match_widest_fix_line_in_file ... ok
test tests::load_custom_dictionaries_keeps_last_duplicate_entry ... ok

test result: ok. 273 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.79s

     Running unittests src/bin/generate_sensitive_tags.rs (target/llvm-cov-target/debug/deps/generate_sensitive_tags-82d554d5920ae079)

running 6 tests
test tests::collect_xml_paths_returns_sorted_xml_files_only ... ok
test tests::load_fields_parses_and_deduplicates_by_tag_number ... ok
test tests::write_output_creates_parent_directory_and_serialises_tags ... ok
test tests::find_repo_root_walks_up_from_nested_directories ... ok
test tests::filter_sensitive_selects_expected_field_names ... ok
test tests::run_generates_sensitive_file_from_repo_resources ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/appendix_d.rs (target/llvm-cov-target/debug/deps/appendix_d-64288bb72a934463)

running 1 test
test generated_appendix_d_corpus_is_present_and_decodes ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.96s

     Running tests/cli.rs (target/llvm-cov-target/debug/deps/cli-17fb2035e65403ea)

running 36 tests
test duplicate_fix_flags_are_rejected ... ok
test explicit_help_ignores_invalid_default_args_env ... ok
test default_args_env_rejects_version_flag ... ok
test explicit_version_ignores_invalid_default_args_env ... ok
test component_detail_verbose_columns_show_fields ... ok
test fixt_logon_default_appl_ver_applies_to_later_messages ... ok
test decodes_single_message_from_stdin ... ok
test default_args_env_applies_cli_flags ... ok
test component_listing_works_in_plain_and_column_modes ... ok
test decodes_message_from_file_path ... ok
test explicit_header_style_renders_source_banner_for_files ... ok
test explicit_cli_args_override_default_args_env ... ok
test file_output_starts_with_the_file_name_even_without_header_style ... ok
test file_decode_prints_separator_before_message_type_summary ... ok
test message_detail_accepts_msg_type_lookup ... ok
test missing_message_is_reported ... ok
test missing_component_is_reported ... ok
test secret_files_mode_writes_valid_obfuscated_sibling_file ... ok
test message_listing_works_in_plain_and_column_modes ... ok
test invalid_and_missing_tags_are_reported ... ok
test number_flag_prefixes_input_lines ... ok
test nocounts_suppresses_message_type_summary ... ok
test query_commands_normalise_fix_key_variants ... ok
test plain_overrides_number_from_default_args_env ... ok
test info_flag_marks_fix27_and_fix30_as_fix40_aliases ... ok
test info_flag_lists_available_dictionaries_and_highlights_selection ... ok
test tag_detail_verbose_columns_show_enum_values ... ok
test summary_mode_ignores_admin_messages ... ok
test summary_mode_highlights_invalid_order_messages_and_surfaces_reason ... ok
test version_flag_prints_only_version_information ... ok
test summary_mode_orders_events_and_collapses_duplicate_order_flow_messages ... ok
test override_is_honoured_with_fallback ... ok
test summary_mode_outputs_order_summary ... ok
test summary_nocounts_suppresses_message_type_summary ... ok
test tag_listing_works_in_plain_and_column_modes ... ok
test validation_reports_missing_fields ... ok

test result: ok. 36 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.72s

     Running tests/repeating_groups.rs (target/llvm-cov-target/debug/deps/repeating_groups-f1148b1b38128acc)

running 1 test
test generated_repeating_group_corpus_is_present_and_validation_clean ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s

     Running unittests src/main.rs (target/llvm-cov-target/debug/deps/pcap2fix-56b683c69cc90f25)

running 22 tests
test tests::flush_complete_messages_skips_midstream_fragment_before_next_message ... ok
test tests::evict_idle_drops_stale_flows ... ok
test tests::flow_shard_pool_orders_results_by_packet_index ... ok
test tests::flush_complete_messages_retains_partial_begin_string ... ok
test tests::append_segment_trims_overlap_and_store_future_segment_prefers_longest ... ok
test tests::flush_complete_messages_resynchronizes_after_leading_garbage ... ok
test tests::flush_complete_messages_emits_messages_and_discards_non_fix_tail ... ok
test tests::find_message_end_rejects_invalid_body_length_and_checksum_fields ... ok
test tests::flow_shard_exits_when_result_channel_is_closed ... ok
test tests::flow_shard_evicts_stale_partial_flows_on_idle_command ... ok
test tests::flush_remaining_flow_outputs_are_sorted_by_flow_key ... ok
test tests::flushes_full_messages_and_discards_non_fix_tail ... ok
test tests::out_of_order_future_segment_is_buffered_until_gap_arrives ... ok
test tests::open_reader_errors_for_missing_file ... ok
test tests::parse_delimiter_variants ... ok
test tests::ipv6_tcp_payload_is_reassembled ... ok
test tests::parse_delimiter_rejects_invalid_values ... ok
test tests::reassembly_overflow_clears_flow_state ... ok
test tests::reassembly_appends_in_order ... ok
test tests::retain_partial_begin_string_clears_non_matching_tail ... ok
test tests::retransmit_is_ignored ... ok
test tests::sweep_idle_flows_dispatches_at_most_once_per_interval ... ok

test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/roundtrip.rs (target/llvm-cov-target/debug/deps/roundtrip-83ebaed19055964f)

running 1 test
test pcap_roundtrip_matches_expected_output ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s


    Finished report saved to target/coverage/coverage.xml
normalised Cobertura report: removed 0 classes and 0 line entries
   Compiling libc v0.2.186
   Compiling proc-macro2 v1.0.103
   Compiling serde_core v1.0.228
   Compiling quote v1.0.42
   Compiling unicode-ident v1.0.22
   Compiling memchr v2.7.6
   Compiling bitflags v2.10.0
   Compiling winnow v1.0.2
   Compiling cfg-if v1.0.4
   Compiling version_check v0.9.5
   Compiling crossbeam-utils v0.8.21
   Compiling objc2 v0.6.3
   Compiling utf8parse v0.2.2
   Compiling toml_parser v1.1.2+spec-1.1.0
   Compiling anstyle-parse v0.2.7
   Compiling objc2-encode v4.1.0
   Compiling colorchoice v1.0.4
   Compiling anstyle v1.0.13
   Compiling parking_lot_core v0.9.12
   Compiling rustix v1.1.2
   Compiling is_terminal_polyfill v1.70.2
   Compiling cfg_aliases v0.2.1
   Compiling anstyle-query v1.1.5
   Compiling signal-hook v0.3.18
   Compiling autocfg v1.5.0
   Compiling num-traits v0.2.19
   Compiling anstream v0.6.21
   Compiling nix v0.30.1
   Compiling anyhow v1.0.100
   Compiling strsim v0.11.1
   Compiling crossbeam-epoch v0.9.18
   Compiling heck v0.5.0
   Compiling getrandom v0.3.4
   Compiling toml_datetime v1.1.1+spec-1.1.0
   Compiling errno v0.3.14
   Compiling serde_spanned v1.1.1
   Compiling signal-hook-registry v1.4.8
   Compiling toml v1.1.2+spec-1.1.0
   Compiling clap_lex v0.7.6
   Compiling semver v1.0.27
   Compiling serde v1.0.228
   Compiling rayon-core v1.13.0
   Compiling syn v2.0.111
   Compiling log v0.4.29
   Compiling scopeguard v1.2.0
   Compiling rustix v0.38.44
   Compiling smallvec v1.15.1
   Compiling winresource v0.1.31
   Compiling mio v1.2.0
   Compiling lock_api v0.4.14
   Compiling block2 v0.6.2
   Compiling rustc_version v0.4.1
   Compiling clap_builder v4.5.53
   Compiling crossbeam-deque v0.8.6
   Compiling aho-corasick v1.1.4
   Compiling core-foundation-sys v0.8.7
   Compiling minimal-lexical v0.2.1
   Compiling regex-syntax v0.8.8
   Compiling nom v7.1.3
   Compiling iana-time-zone v0.1.64
   Compiling fixdecoder v0.3.0 (.)
   Compiling clap_derive v4.5.49
   Compiling serde_derive v1.0.228
   Compiling regex-automata v0.4.13
   Compiling dispatch2 v0.3.0
   Compiling signal-hook-mio v0.2.5
   Compiling parking_lot v0.12.5
   Compiling thiserror v1.0.69
   Compiling fastrand v2.3.0
   Compiling either v1.15.0
   Compiling once_cell v1.21.3
   Compiling rayon v1.11.0
   Compiling clap v4.5.53
   Compiling tempfile v3.23.0
   Compiling crossterm v0.28.1
   Compiling ctrlc v3.5.1
   Compiling rusticata-macros v4.1.0
   Compiling chrono v0.4.42
   Compiling thiserror-impl v1.0.69
   Compiling terminal_size v0.4.3
   Compiling pcap2fix v0.1.0 (pcap2fix)
warning: fixdecoder@0.3.0: Building fixdecoder v0.3.0 (branch:main, commit:f4ba8ce) [rust:1.96.0]
   Compiling roxmltree v0.21.1
   Compiling regex v1.12.2
   Compiling quick-xml v0.36.2
   Compiling shlex v1.3.0
   Compiling arrayvec v0.7.6
   Compiling circular v0.3.0
   Compiling etherparse v0.15.0
   Compiling pcap-parser v0.14.1
    Finished `release` profile [optimized] target(s) in 14.78s
make[1]: Leaving directory '.'
```

Build only the optimized release binaries.

```bash
❯ make build-release

>> Ensuring Rust toolchain and coverage tools
>> Ensuring FIX XML specs are present
    Finished `release` profile [optimized] target(s) in ...
```

Run it (from the optimized build) and check the version details:

```bash
❯ ./target/release/fixdecoder --version
fixdecoder 0.3.0 (branch:main, commit:f4ba8ce) [rust:1.96.0]
```

<!-- regen-readme:end --section=build-examples -->

# PCAP to FIX filter (`pcap2fix`)

The workspace includes a helper that reassembles TCP streams from PCAP data and emits FIX messages to stdout so you can pipe them into `fixdecoder`. I have wrapped it in a shell script (`./scripts/capture_and_decode.sh`) to make it easy to run.

- Build: `cargo build -p pcap2fix` (also built via `make build`).
- Offline: `pcap2fix --input capture.pcap | fixdecoder`
- Live (needs tcpdump/dumpcap): `tcpdump -i eth0 -w - 'tcp port 9876' | pcap2fix --port 9876 | fixdecoder`
- Delimiter defaults to SOH; override with `--delimiter`.
- Flow buffers are capped (size + idle timeout) to avoid runaway memory during long captures.

![Capture and Decode](docs/capture_and_decode.png)

# Technical Notes on the use of the `--summary` flag

- As messages stream by, the decoder builds one “record” per order (keyed by OrderID/ClOrdID/OrigClOrdID).
- Each message updates that record: standard fields (Side, Symbol, Qty, Price, TIF, OrdType, TradeDate, SettlDate) are taken from the latest message; BN messages also set ExecAckStatus, Spot Price (LastPx), and ExecAmt (38).
- The header row shows the order key, the flow of states observed (OrdStatus/ExecType/ExecAckStatus), and a table of the latest known values: Side/Symbol/Qty/Price/TradeDate/Tenor/TIF/OrdType/ValueDate (tag 64/193). Prices include currency when present.
- The timeline lists every message for the order with columns: time, msg (enum text plus ClOrdID/OrigClOrdID), ExecAckStatus (for BN), ExecType, OrdStatus, cum/leaves, last@price, avgPx, text. Enums show text; unknown codes show in red; missing text shows as “-” in green.
- Tenor is computed from TradeDate to ValueDate skipping weekends; SPOT = T+2, TOM = T+1, TOD = T+0, otherwise FWD. (no holiday calendars).
- If a `--fix` override cannot be found, decoding falls back to the auto-detected dictionary with a warning on stderr and a banner at runtime.

# Third-Party Specifications

This project uses the public FIX Protocol XML specifications from the
[QuickFIX project](https://github.com/quickfix/quickfix/tree/master/spec).
The XML files are downloaded during the build and used to generate Go sources
under `fix/` and to drive message decoding at runtime.

The QuickFIX specifications are licensed under the **BSD 2-Clause License**.
Their copyright notice and license terms are included in this repository’s
[`NOTICE`](./NOTICE) file (and in `licenses/QUICKFIX-BSD-2-Clause.txt`).

---

© 2025 Steve Clarke · Released under the [AGPL-3.0 License](https://www.gnu.org/licenses/agpl-3.0.html)

---

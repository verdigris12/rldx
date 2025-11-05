
**Deterministic merge policy (canonical-first, in-place)**

* **Inputs.** A nonempty, already-sorted list `V = [C, D1, …, Dn]`. Keep `C` (the canonical card) and update it in place; delete `D*` after a successful merge. Always preserve `C.UID`. Update `REV` at the end.

* **Parsing & prep.**

  * Parse all cards with `vcard4`.
  * Build normalizers:

    * TEL → E.164 via `rlibphonenumber` (fallback region from config).
    * EMAIL → trim, collapse whitespace, lowercase **domain** only; keep local-part verbatim; NFC/NFKC normalize strings where appropriate (`unicode-normalization`).
    * IMPP/URL → parse with `url`, punycode/IDNA handled by `url` (or `idna` if needed).
  * Treat these as **multi-valued** sets: `TEL`, `EMAIL`, `IMPP`, `URL`, `NICKNAME`, `ADR`, `PHOTO`/`LOGO`.

* **Per-card merge (for each `D` in order).**

  * **Unknown/vendor data.** Preserve all `X-*` properties and Apple `itemN.*` groups; don’t drop data you don’t understand.
  * **Property identity.** Keep existing `PID`s on properties already in `C`. For properties imported from `D`, **strip** `PID` (safer than minting new ones) and clear/adjust `CLIENTPIDMAP` accordingly later.
  * **Name/display.**

    * Keep `C.FN` as the display name.
    * For `N`: if `C.N` is missing or less complete (fewer filled components) than `D.N`, replace; otherwise keep `C.N`.
    * If `D` provides alternate language/script forms of `FN`/`N`, add them using matching `ALTID` (same `ALTID` between `FN` and `N`) and `LANGUAGE`; leave `C`’s current pair as `PREF=1`.
  * **Nicknames.** `NICKNAME := uniq( C ∪ D )` using case-folded, trimmed tokens; keep original casing on write-out.
  * **Notes.** Append `D.NOTE` to `C.NOTE` separated by a blank line. Deduplicate exact paragraphs; preserve formatting.
  * **TEL.**

    * Normalize both; dedup by `(E164, ext)` key.
    * Union `TYPE`s when values match; drop duplicate `PREF`s.
    * Keep **all** distinct numbers. Use `PREF=1` for C’s current primary; ensure no more than one `PREF=1`. For additional “same kind” numbers, **do not invent `TYPE=cell2`** (not standard). Keep multiple `TEL` with `TYPE=CELL` and assign `PREF=2,3,…` as needed. If you insist on a suffix label, use `TYPE=X-CELL-2` (vendor-ext).
  * **EMAIL.**

    * Normalize; dedup by `(local, domain)` tuple; union `TYPE`s.
    * Keep all distinct emails; ensure only one has `PREF=1` (prefer C’s).
  * **IMPP.**

    * Require a valid URI. Normalize scheme/host; dedup by the canonical URI string.
    * Preserve any `TYPE` that identifies the service (e.g., `TYPE=telegram|matrix|xmpp`); keep all distinct URIs; single `PREF=1`.
  * **URL.** Normalize with `url`; dedup; keep all; one `PREF=1`.
  * **ADR.** Normalize component-wise (trim, collapse spaces). Dedup by normalized tuple; union `TYPE`s; keep all; one `PREF=1`.
  * **ORG/TITLE/ROLE/GENDER/BDAY/ANNIVERSARY/TZ/GEO/KIND/etc.** Prefer `C` unless `D` is strictly more complete or newer by `REV`; otherwise keep `C`. Do not duplicate these scalars.
  * **PHOTO/LOGO.**

    * If inline `DATA`: compute digest (e.g., `sha1`) and dimensions (`image` crate). Prefer the higher-resolution or larger file if same media type; dedup by digest.
    * If `VALUE=URI`: dedup by canonicalized URL; prefer highest-resolution when detectable (heuristic).
    * Keep at most one inline + one URI variant (if distinct).
  * **LABEL/PARAMS hygiene.** Preserve existing `TYPE`, `PREF`, `LANGUAGE`, `ALTID`, `LABEL`, groups (`itemN`), merging `TYPE`s rather than overwriting.

* **Post-merge fixes.**

  * **PREF integrity.** For each multi-valued property class (`TEL`, `EMAIL`, `IMPP`, `URL`, `ADR`, and the set of parallel `FN`/`N` pairs), ensure **exactly one** `PREF=1`. Remove or renumber others (`PREF=2,3,…`) deterministically (canonical first, then by sort key).
  * **CLIENTPIDMAP.** If you kept any `PID`s, retain/compact `CLIENTPIDMAP` to only referenced entries; otherwise drop it.
  * **REV.** Set to current timestamp.
  * **Sorting.** Optionally set/update `SORT-AS` for `N`/`ORG` if you maintain it.

* **Deletion of duplicates.**

  * Persist the updated `C` to storage.
  * Delete `D*`. If using CardDAV (`libdav` feature), issue `DELETE` with `If-Match` against each `D`’s ETag to avoid races.

**Normalization & parsing crates (prefer your existing stack):**

* Phone: `rlibphonenumber` (you already use it) for E.164 + extension parsing.
* Email: add `email_address` (robust RFC 5322/6531 parser). Lowercase **domain** only; don’t over-normalize local-part. If you need looser parsing, `addr` is an alternative.
* URIs (IMPP/URL): `url` (covers IDNA); use it to canonicalize and compare.
* Unicode cleanup: `unicode-normalization` for NFC/NFKC; optionally `icu_casemap` for locale-aware casefolding.
* Images: your `image` crate for dimensions/format sniffing.
* Hashing: your `sha1` for PHOTO/LOGO dedup (or switch to `sha2` if you want stronger digests).
* Timestamps: your `time` crate to write `REV`.

**Notes on standards compliance.**

* Don’t invent new `TYPE` tokens like `cell2`; disambiguate with multiple repeated properties and `PREF` (standards-compliant). If you must label the second “cell,” use `TYPE=X-CELL-2` (explicit vendor extension) or a `LABEL` parameter.



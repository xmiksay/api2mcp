# MCP Tool Factory — implementační plán

## 1. Co to je

Služba, která z HTTP API dělá kurátorované MCP tooly. Tři úrovně použití:

1. **Deklarativní** — člověk nebo agent definuje api_call a vystaví ho jako tool.
2. **Kompozitní** — skript složí několik api_callů do jedné odpovědi použitelné modelem.
3. **Sdílené** — definice jsou přenositelný artefakt; příjemce si doplní vlastní službu a credentials.

Hodnota není v tom, že agent získá schopnost volat API (to umí s jedním HTTP toolem), ale v **persistenci, determinismu, zúžení a auditovatelnosti** té schopnosti.

## Co to není

- Není to agregátor existujících MCP serverů. Ty, kdo mají oficiální MCP server (GitLab, GitHub, Datadog), nezastupuje.
- Není to generický OpenAPI→MCP generátor. Import specu je scaffolder, ne runtime.
- Není to hosting cizích credentials. Sdílí se artefakt, ne endpoint.

---

## 2. Invarianty

Platí ve všech fázích. Když se nějaká fáze dostane do konfliktu s invariantem, mění se fáze, ne invariant.

| # | Invariant |
|---|---|
| I1 | Skript nikdy nedělá HTTP. Volá výhradně api_cally deklarované v témže rozsahu. |
| I2 | Množina originů, kam může endpoint volat, je spočitatelná staticky před spuštěním. |
| I3 | URL šablona je fixní v okamžiku definice. Parametry plní placeholdery a nemohou změnit schéma, host ani strukturu cesty. |
| I4 | Credential není nikdy dosažitelný modelem — ani v odpovědi, ani v chybě, ani v logu vráceném do kontextu. |
| I5 | Vazba auth provider ↔ origin nastavuje člověk. Agent ji nemůže navrhnout ani změnit. |
| I6 | Rozpočty (počet volání, bajty, wall clock) vynucuje runtime, ne skript. |
| I7 | Exekuce je deterministická: bez hodin, bez randomu, stabilní pořadí iterací i výsledků fan-outu. |
| I8 | Jméno toolu je verzovaný kontrakt. Definice se needituje in-place, vzniká nová verze. |

---

## 3. Datový model

Pět entit. Každá verzovaná, neměnná po publikaci.

**service** — instance cílového systému. `base_url`, allowlist originů, rate limit, výchozí hlavičky, vazba na auth providery. Nositel toho, co se při sdílení mění (URL, token) a co při importu doplňuje příjemce.

**auth_provider** — způsob autentizace vůči službě. Typ (bearer / API key v hlavičce / OAuth), reference na credential v trezoru, deklarované scopes. Váže se na origin, ne na api_call.

**api_call** — jedna operace. Metoda, šablona cesty, vstupní schéma (zúžené, s defaulty a fixními parametry), projekce odpovědi, klasifikace read/write, idempotence, tagy, test fixture.

**script** — kompozice. Rhai, vstupní schéma, výstupní schéma, seznam api_callů, které smí volat, rozpočty, test fixture.

**endpoint** — to, co vidí agent. Výběr api_callů a skriptů výrazem nad tagy, aliasy jmen, rozpočty, read/write strop, rozsah dostupných auth providerů, pin na verze.

Skupiny řeš **tagy (many-to-many)**, ne stromem. Tag vzniká ze služby, domény a read/write; endpoint vybírá výrazem.

---

## 4. Fáze

Odhady jsou hrubé a předpokládají soustředěnou práci.

### Fáze 1 — Exekuční jádro (3–4 dny)

Bez MCP, bez UI, bez trezoru. Konfigurace jako YAML v gitu.

- service, auth_provider (zatím token z env), api_call
- HTTP klient: šablony, validace vstupu proti schématu, projekce odpovědi, redakce hlaviček
- Vynucení allowlistu originů, SSRF ochrana (privátní rozsahy, metadata endpointy, DNS rebinding)
- Rozpočty na velikost odpovědi a wall clock
- CLI: spusť jeden api_call, ukaž syrovou i projektovanou odpověď

**Exit:** jeden reálný api_call proti reálné službě vrací ořezanou odpověď; pokus o volání mimo allowlist je odmítnutý před odesláním.

### Fáze 2 — MCP data plane (2–3 dny)

- Streamable HTTP server, `tools/list`, `tools/call`
- Endpoint entita, výběr tagy, aliasy jmen
- Statický bearer na vstupu (OAuth odloženo)
- Mapování chyb na MCP chybové odpovědi

**Exit:** Claude Code se připojí a zavolá tool.

**Rozhodovací bod:** na jedné reálné úloze porovnat s existujícím řešením (oficiální GitLab MCP server, nebo holý `glab` v bashi). Pokud kurátorovaný endpoint nevyhrává v počtu tokenů ani v úspěšnosti, projekt nemá smysl stavět dál v téhle podobě.

### Fáze 3 — Skripty (4–5 dnů)

Tady je největší technické riziko i největší hodnota.

- Rhai na `new_raw()` engine, bez stdlib, s limity na operace, hloubku a velikost
- Host funkce: `call(name, args)` a `call_many(name, args_list)` — pouze na deklarované api_cally (I1)
- Fan-out v Rustu: paralelní, limit souběžnosti **per service**, výsledky v pořadí vstupů (I7)
- Rozpočty: max volání, max bajtů kumulativně, wall clock, max stránek při paginaci
- Sémantika částečného selhání: částečný výsledek + explicitní pole s chybami, nikdy hard fail při jednom neúspěšném callu
- Chybová hlášení s číslem řádku a mezivýsledky

**Exit:** jeden multi-call tool s reálnou hodnotou (např. celý kontext pro review MR: metadata + diff + komentáře + stav pipeline v jedné odpovědi), který běží deterministicky proti test fixture.

### Fáze 4 — Trezor a auth (3–4 dny)

- Envelope encryption: náhodný DEK na credential, DEK zabalený KEKem
- **Dvě třídy credentials:**
  - *user* — KEK odvozený z uživatelského tajemství, dešifrovatelné jen za běhu requestu
  - *service* — KEK serverový (age / KMS), pro headless běhy (webhooky, cron, GitLab agent)
- Oddělení přihlašovacího tokenu od unwrap secretu (revokovatelnost bez přešifrování)
- OAuth refresh mimo request uživatele → jen pro service credentials
- Rotace KEKu přebaluje DEKy, ne credentials

**Exit:** headless běh funguje bez uživatele v letu; rotace klíče proběhne bez přešifrování trezoru; token se neobjeví v žádném logu ani odpovědi.

### Fáze 5 — Control plane, agent-authored tooly (4–6 dnů)

Oddělený endpoint od data plane, aby továrna neseděla v kontextu konzumujících agentů.

Životní cyklus: **discover → draft → test → promote**

- `discover` — sonda přes omezený call (allowlisted host, jen GET, uříznutá odpověď); agent se učí tvary z reálných dat
- `draft` — agent píše jméno, popis, schéma, šablonu, projekci; validace: schéma well-formed, jméno unikátní, projekce se aplikuje na vzorek z discover
- `test` — povinná brána, bez jednoho úspěšného volání tool nevznikne; výsledek se uloží jako test fixture
- `promote` — scope session / agent / tenant; cokoliv nad session schvaluje člověk
- Karanténa: nový tool je session-only, read-only, bez persistence
- Zápisové operace agent neregistruje (v této fázi vůbec)
- `invoke(tool_name, args)` dispatcher jako fallback pro klienty, které ignorují `notifications/tools/list_changed`
- GC: TTL, počítadla použití, telemetrie chybovosti → návrh na retire nebo přepis

**Exit:** agent si sám vytvoří funkční tool a použije ho ve stejném sezení.

### Fáze 6 — Sdílení (5–7 dnů)

- Formát manifestu: implementačně nezávislý, deklarativní, bez spustitelného kódu mimo čisté transformace
- Manifest deklaruje **třídu credentialu a požadované scopes**, nikdy credential ani jeho referenci
- Proměnné instance (`base_url`, namespace) vyplňuje příjemce
- Test fixtures cestují s definicí — příjemce ověří funkčnost proti své instanci před promote
- Jednotka sdílení je **pack** (sada toolů), ne jednotlivý tool
- Neměnné hashované verze, pin, re-review při updatu; žádný auto-update
- Podpis a provenience
- Import review: popisy toolů se zobrazí člověku jako **nedůvěryhodný obsah** (jsou to instrukce, které jdou modelu do kontextu)
- Importované packy: jen čisté transformace bez I/O + deklarativní pipeline. Volná orchestrační smyčka jen pro lokálně psané tooly.

**Exit:** pack vyexportovaný z jedné instance běží na druhé po doplnění služby a tokenu.

---

## 5. Otevřená rozhodnutí

Tyhle je potřeba rozhodnout, ne odložit — každé mění rozsah:

1. **Kdo je konzument?** Vlastní runtime (pak `list_changed` funguje a fáze 5 je jednodušší) vs. Claude Code a cizí klienti (pak je `invoke` dispatcher nutnost, ne fallback).
2. **Sdílení uvnitř týmu, nebo mezi cizími?** Uvnitř: supply chain řeší git a code review, fáze 6 se smrskne na polovinu. Mezi cizími: ověřovací a review pipeline **je** ten produkt.
3. **Zápisové operace v rozsahu?** Pokud ano, kdy — a s jakým approval flow.
4. **Multi-tenant, nebo single-tenant?** Rozhoduje o trezoru, izolaci a kvótách. Doporučení: single-tenant self-hosted, sdílení přes artefakty.

---

## 6. Rizika

| Riziko | Dopad | Mitigace |
|---|---|---|
| Klienti ignorují `list_changed` | Fáze 5 nefunguje end-to-end | `invoke` dispatcher; ověřit brzy na cílovém klientovi |
| Projekt duplikuje existující řešení | Zahozená práce | Rozhodovací bod na konci fáze 2 |
| Skript jako exfiltrační kanál | Kritické | I1 + I2 + I5, importované packy bez volné orchestrace |
| Popis toolu jako injection vektor | Kritické při sdílení | Popisy jako nedůvěryhodný obsah v import review |
| Bobtnání tool listu | Návrat původního problému | GC, telemetrie, packy jako jednotka instalace |
| Trezor nefunguje headless | Blokuje vlastní use-case | Rozdělení user/service credentials ve fázi 4 |

---

## 7. Odloženo

- **Delegace tokenu (RFC 8693 token exchange).** Správný cíl, ale mimo současný rozsah. Design nesmí bránit pozdějšímu doplnění: auth_provider musí být samostatná entita s deklarovanými scopes, aby se dala nahradit exchange flow bez zásahu do api_callů.
- OAuth 2.1 na vstupu do gateway (DCR, protected resource metadata).
- GraphQL a gRPC upstreamy.
- Web UI — konfigurace zůstává v gitu.
- Rune jako alternativa k Rhai — zvážit, až kdyby `call_many` přestalo stačit.

---

## 8. Kde se implementace vědomě odchýlila od tohoto plánu

Tento dokument je historický záznam původního záměru a **nepřepisuje se**. Skutečný stav je v
`src/lib.rs` (tabulka invariantů), `docs/architecture.md` a `docs/scripting.md`. Níže je seznam
míst, kde se implementace vědomě rozešla s tím, co je napsáno výše, a proč.

- **I8 (verzované, neměnné definice) nebyl implementován.** Definice jsou mutable a "last write
  wins" — editace přepíše řádek na místě, žádná nová verze nevzniká. Historii nese výhradně run
  log: každý `runs` řádek nese kompletní `definition_snapshot` (a jeho `definition_digest`) toho,
  co se skutečně spustilo, takže "co dělal tenhle tool, když běžel" je vždy zodpověditelné i bez
  verzování. Tahle záměna kupuje mnohem menší write path výměnou za ztrátu možnosti "připnout"
  konkrétní verzi toolu; zbytek I8 postihujících úvah (jméno toolu jako kontrakt) zůstává v duchu
  zachován tím, že přejmenování/změna slugu je vždy nová definice, ne editace staré.
- **`call`/`call_many` se jmenují `api`/`api_many`** (plus nově `api_try`, který plán nezmiňuje —
  viz níže). `call` je v Rhai vyhrazené klíčové slovo interpretu (`KEYWORD_FN_PTR_CALL`) — stejnojmenná
  `register_fn` by byla tiše zastíněna, ne odmítnuta při registraci. Přejmenování bylo jediná
  bezpečná oprava; `call_many` → `api_many` následovalo pro konzistenci pojmenování. `api_try`
  přibyl navíc: stejné jedno volání jako `api`, ale nikdy nevyhazuje výjimku — vrací tvar
  per-item výsledku (`ok`/`index`/`value`|`error`), takže skript může použít jednotný idiom
  ošetřování chyb napříč `api`/`api_many`/`api_try`, místo `try`/`catch` bloku jen kolem `api`.
- **Postgres je zdroj pravdy, ne YAML v gitu.** Plán (§6) počítal s tím, že sdílení je jednotka
  "pack" a definice žijí primárně jako YAML v repozitáři. Místo toho je zdroj pravdy databáze —
  YAML pack (`pack::export`/`pack::import`) je čistě přenosný, credential-free formát pro přesun
  definic mezi instancemi nebo pro `examples/demo.pack.yaml`, ne úložiště, ze kterého by se za
  běhu četlo. Důvod je praktický: admin API (viz níže) potřebuje editovat definice za běhu, s
  okamžitou platností a bez commitu/deploy cyklu — to jde jen proti databázi.
- **Single-tenant nahrazen per-uživatelským vlastnictvím (`owner_id`).** Otevřené rozhodnutí č. 4
  ("Multi-tenant, nebo single-tenant?") bylo nakonec rozhodnuto ve prospěch multi-tenantu: každá
  z pěti definičních entit (`service`, `auth_provider`, `api_call`, `script`, `endpoint`) nese
  reálný `owner_id` sloupec, slugy jsou unikátní per-owner (ne globálně), a sdílení mezi vlastníky
  zůstává explicitně mimo databázi — přes export/import YAML packu, přesně jak plán zamýšlel pro
  sdílení mezi cizími (fáze 6), jen teď platí i uvnitř jedné instance mezi jejími vlastními účty.
  Trezor (fáze 4, envelope encryption dvou tříd credentials) se nerealizoval v této podobě —
  credential je env var *jméno* na `auth_providers` řádku, nikdy hodnota v databázi; hodnota žije
  jen v procesním prostředí serveru.
- **Admin/non-admin rozlišení bylo zavedeno a pak zase zrušeno.** Fáze 5 plánu předpokládala
  odlišný "control plane" endpoint pro agenta, který si sám vytváří tooly (`discover` → `draft` →
  `test` → `promote`), s schvalováním nad úrovní session člověkem. To se nerealizovalo; místo toho
  vznikl web UI (viz další bod) ovládaný přímo člověkem, a jediné rozlišení, které zůstalo, je mezi
  "session/CLI" (může číst a psát vlastní definice) a "service token/OAuth" (může jen volat tooly
  přes `/mcp`) — žádná role "admin" mezi lidmi neexistuje.
- **Web UI bylo postaveno, ne odloženo.** Sekce 7 ("Odloženo") výše řadí web UI mezi věci mimo
  rozsah s poznámkou "konfigurace zůstává v gitu" — to platilo, dokud byl zdroj pravdy YAML v
  gitu (viz výše). Jakmile se zdrojem pravdy stal Postgres, potřeba administrativního UI nad ním
  přestala být odložitelná: `server::api` je čitelné a zapisovatelné JSON API nad každou definiční
  entitou (CRUD + testovací spuštění `POST /api/{api_calls,scripts}/{slug}/test`) a Vue 3 SPA nad
  ním je zabudované přímo do binárky (`rust-embed`), takže nasazení zůstává jeden soubor.
- **I7 bylo přeformulováno.** Původní znění ("bez hodin, bez randomu") bylo nutné zmírnit, protože
  skript potřebuje reálnou práci s datem/časem, aby byl k něčemu při transformaci dat. Mechanická
  půlka I7 — stabilní pořadí iterace, výsledky fan-outu ve vstupním pořadí, rozpočty připisované
  v indexovém pořadí — zůstává bezpodmínečná a nezměněná. Do skriptu nově vstupují dvě oddělené,
  explicitně pojmenované funkce: `execution_start()` (jeden zmrazený okamžik, konstantní po celý
  běh — reprodukovatelné) a `now()` (živé hodiny reálného času — nereprodukovatelné). Volba mezi
  nimi je viditelná přímo ve zdrojovém textu skriptu, ne skrytá uvnitř enginu. `BasicTimePackage`
  (a s ním `timestamp()`) zůstává vyloučen ze stejného důvodu, pro který ho I7 v původním znění
  zakazovalo úplně — hlídá to golden test nad registrovanou sadou funkcí enginu.

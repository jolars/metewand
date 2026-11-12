# Changelog

## [0.6.0](https://github.com/jolars/metewand/compare/v0.5.0...v0.6.0) (2026-11-12)

### Features

- support default profile bindings ([`e35f1c0`](https://github.com/jolars/metewand/commit/e35f1c0e3e64bfd3d2c429f0154bd1509d12173c))
- allow overrides for compact parameter defaults ([`48a3982`](https://github.com/jolars/metewand/commit/48a3982e8a5f3f0987af68e04cf4892eb5aa073f))
- **ui:** nest datasets in layout ([`c5abad7`](https://github.com/jolars/metewand/commit/c5abad756ab9cc6a270052021d4f2b9f9fd0e29b))
- randomize implementations within datasets ([`8495a73`](https://github.com/jolars/metewand/commit/8495a735c0631349c7855479b827ffd3935cd179))
- **ui:** improve interactive output ([`bd313d1`](https://github.com/jolars/metewand/commit/bd313d1708c636b4f232614c3905bb071d67a841))
- filter benchmark components by name ([`5b381e0`](https://github.com/jolars/metewand/commit/5b381e0d1c2d9319ad1a442ee6bc6d7b257ac13e))
- accept positional experiment names ([`ad8a962`](https://github.com/jolars/metewand/commit/ad8a96279b1071660c50339828338cb05c7b98f6))
- default runs to `.metewand` ([`3ed562e`](https://github.com/jolars/metewand/commit/3ed562ebb3fca93ad456b479af6156b997283090))
- resume existing runs by default ([`f856470`](https://github.com/jolars/metewand/commit/f8564703d8a0c1b546a91a2ea85fd199cbaeed61))
- render benchmark progress with iocraft ([`f1eb214`](https://github.com/jolars/metewand/commit/f1eb214a22747b591c908f45d5d390a70622ecc1))
- add live benchmark progress bars ([`16d0d35`](https://github.com/jolars/metewand/commit/16d0d3591fa2f5f1d9f852537069280062cf4cd4))
- add local Apptainer SIF execution ([`dc9cb42`](https://github.com/jolars/metewand/commit/dc9cb428ea94625912abe74d63ae33ade0151079))
- add whole-benchmark bundles ([`355ec13`](https://github.com/jolars/metewand/commit/355ec130885e25d28357206ccc6fcf7320a1777d))
- add digest-pinned Docker execution ([`a161893`](https://github.com/jolars/metewand/commit/a161893a96bd30914356661091fff6242dd9a50b))

### Bug Fixes

- **ui:** better table formatting ([`d4fd01d`](https://github.com/jolars/metewand/commit/d4fd01d2804f2362eaf627c6da7df765f6efb310))
- summarize human plan output by default ([`462c011`](https://github.com/jolars/metewand/commit/462c011f3f4443da06eeaceac9976ad7af8e023a))
- correct YAML for CI integration test commands ([`d162a9f`](https://github.com/jolars/metewand/commit/d162a9f3bb9daf846b4ff133cb88bfa7fcf9b313))
- **ci:** use supported ARM macOS release runner ([`f2c2279`](https://github.com/jolars/metewand/commit/f2c22797bd4235dc3ccf03e5c65f7dec4361a059))

## [0.5.0](https://github.com/jolars/metewand/compare/v0.4.0...v0.5.0) (2026-11-10)

### Features

- support primary computation timing ([`92fba38`](https://github.com/jolars/metewand/commit/92fba38893970d3d6bcd3f196d1bdac78591271b))

## [0.4.0](https://github.com/jolars/metewand/compare/v0.3.0...v0.4.0) (2026-11-05)

### Features

- simplify benchmark authoring ([`1b29eb5`](https://github.com/jolars/metewand/commit/1b29eb59648f48b462744b3d2860ecde6287e4f7))
- add shared problem preparation ([`fbd1959`](https://github.com/jolars/metewand/commit/fbd195973b49950f98ab7549c2c29eab4c1749a8)), closes [#17](https://github.com/jolars/metewand/issues/17)
- add worker timeouts and explicit checkpoint schedules ([`e12831e`](https://github.com/jolars/metewand/commit/e12831e111ce208a0be1d85a065a9d15c7874571))

### Bug Fixes

- preserve Windows worker environment templates ([`7205589`](https://github.com/jolars/metewand/commit/7205589e87a9b031cab8b79a62e52f8cd375dda1))
- allow output outside nonrecursive source globs ([`348e77b`](https://github.com/jolars/metewand/commit/348e77b42b91af894706728a22af37d06f2cb6d7)), fixes [#16](https://github.com/jolars/metewand/issues/16)
- check formatting of shared Rust modules ([`6ba13be`](https://github.com/jolars/metewand/commit/6ba13bea3553fadb222ef65ba63d954d36fb8bf6)), fixes [#14](https://github.com/jolars/metewand/issues/14)
- bind worker deadlines to resolved identities ([`794ca43`](https://github.com/jolars/metewand/commit/794ca431978a89278af85e0b04abc5b1cc0d0291)), closes [#12](https://github.com/jolars/metewand/issues/12)

## [0.3.0](https://github.com/jolars/metewand/compare/v0.2.0...v0.3.0) (2026-11-02)

### Breaking changes

- move language adapters into the CLI ([`3e03be4`](https://github.com/jolars/metewand/commit/3e03be48dba8621b832655b5a89b184defb460d3))

### Features

- add standalone analysis exports ([`4760a6f`](https://github.com/jolars/metewand/commit/4760a6fb2a01b439d57036f551be6ee6711265ea))
- add native uv locked environments ([`6fb3104`](https://github.com/jolars/metewand/commit/6fb3104340a0c1063a360958cd23c9be9e792570))
- add Nix environment locking ([`98db3e9`](https://github.com/jolars/metewand/commit/98db3e92fc8087a0ea14b7768e4160fb53bd4f04))
- add portable result archives ([`32a1128`](https://github.com/jolars/metewand/commit/32a1128c95c3f209b3e4a878c617fc78a866c48a))
- add streaming profiles ([`651f15d`](https://github.com/jolars/metewand/commit/651f15de43341660321ef38bf85b8d3430969b08))
- release binaries ([`f6ce5e0`](https://github.com/jolars/metewand/commit/f6ce5e0c183c023c75225ad15911c975266387c2))
- support native Windows execution ([`6ef978d`](https://github.com/jolars/metewand/commit/6ef978d3ed62cdb0371d987e71092a2eac6146c3))

### Bug Fixes

- normalize Windows worker directory paths ([`6b953f1`](https://github.com/jolars/metewand/commit/6b953f1e90e7f9ab41581b9b3cdab06ae9ad0b02))
- adapt crash tracing to rusqlite 0.41 ([`befde83`](https://github.com/jolars/metewand/commit/befde8330a655b8b25764a04386809a6b0897bc5))
- repair Windows CI paths and store initialization ([`71e2c62`](https://github.com/jolars/metewand/commit/71e2c62a29a8b4e1a37896b60f6636aeeda2fb54))
- repair macOS and Windows CI failures ([`ae6d68a`](https://github.com/jolars/metewand/commit/ae6d68a60345011e83d3125df91fbfb56f5b2b52))
- restore cross-platform CI ([`6b63d82`](https://github.com/jolars/metewand/commit/6b63d82ebcf6c596227cb42e617b70cee32bd8de))

## [0.2.0](https://github.com/jolars/metewand/compare/v0.1.0...v0.2.0) (2026-10-27)

### Breaking changes

- switch configuration from TOML to YAML ([`b9381eb`](https://github.com/jolars/metewand/commit/b9381ebc546f741bc6270b7ed5ddad582ff4d635))

### Features

- simplify benchmark authoring ([`91cc62d`](https://github.com/jolars/metewand/commit/91cc62daa19afd9180a4132ea3575249b8ce042e))
- qualify local adopter comparisons ([`d5432d7`](https://github.com/jolars/metewand/commit/d5432d77555a521dd2cdfc1b6bcd190533708679))
- add fresh-sequence profiles ([`b372623`](https://github.com/jolars/metewand/commit/b3726232ff08ade594e2b0378cb720506e231250))
- add buffered profiles and local observers ([`712edb4`](https://github.com/jolars/metewand/commit/712edb49db73cc8f643d279856ee94c0759ea280))
- check configuration-dependent applicability ([`c03fa2d`](https://github.com/jolars/metewand/commit/c03fa2dda1b27e436b56a87955059fd47e899078))
- validate Benchopt SLOPE authoring workflow ([`e8456a8`](https://github.com/jolars/metewand/commit/e8456a81ace897637bdf8578f54c919220674456))
- generate compact SDK scaffolds ([`b302b9b`](https://github.com/jolars/metewand/commit/b302b9bea889168e412cac7000c58b7c82438137))
- simplify worker adapters across languages ([`5f9f90c`](https://github.com/jolars/metewand/commit/5f9f90ca718a8859604601b06ca4ce94adf6452c))
- pass declared scientific rules to evaluators ([`916c1f3`](https://github.com/jolars/metewand/commit/916c1f3d60fc4e2ed7c8e31bb50c714871888ed6))
- finish compact parameter overrides ([`eb03a29`](https://github.com/jolars/metewand/commit/eb03a29954edfa666d2275e68fbe8b521e27317b))
- support flat problem declarations ([`85ac6d1`](https://github.com/jolars/metewand/commit/85ac6d1cb9e6a2ceb412f5fb5d45595f8908528b))
- resolve compact authoring defaults ([`0518a3a`](https://github.com/jolars/metewand/commit/0518a3ac5971d740e5283cbbc4c26c0b16ade359))
- extend compact declarations and default grids ([`299a228`](https://github.com/jolars/metewand/commit/299a228a15a3d9f6df9d456328d524a01d27fbb9))
- add compact implementation parameters ([`620a934`](https://github.com/jolars/metewand/commit/620a9342704b12b2d0a411a0290b4274ded5bbe4))
- support inline contracts and schemas ([`66e87bb`](https://github.com/jolars/metewand/commit/66e87bbf101e91186942a86628b3d69e0a1a3d54))
- use kebab-case for YAML configuration keys ([`85548eb`](https://github.com/jolars/metewand/commit/85548eba7d08c858d71e4951c21271956d8c7f7f))
- switch configuration from TOML to YAML ([`b9381eb`](https://github.com/jolars/metewand/commit/b9381ebc546f741bc6270b7ed5ddad582ff4d635))
- add representative local benchmark examples ([`913207b`](https://github.com/jolars/metewand/commit/913207bdfabdbe067008702028fef0b307cf8dc1))
- add cache verification and garbage collection ([`d8625a0`](https://github.com/jolars/metewand/commit/d8625a057c94fc0ee4846723abbd81a05c8a0737))
- add rebuildable indexes and tidy exports ([`5cdd369`](https://github.com/jolars/metewand/commit/5cdd36988aeaad2764bed4608cf45924c2db7ea8))
- verify workers and problem reference cases ([`d58036f`](https://github.com/jolars/metewand/commit/d58036f30cf8a4314b280ed6090ae428c3322e94))
- **cli:** add component scaffolds ([`aa9c6ae`](https://github.com/jolars/metewand/commit/aa9c6ae02d81050b2d8a6044695b13ad1d8ce417))
- **cli:** add `init` command ([`da747d1`](https://github.com/jolars/metewand/commit/da747d1433d5734d083875b0407b77490c22f997))
- **sdk:** add R worker helpers ([`0cc1fb3`](https://github.com/jolars/metewand/commit/0cc1fb3bdff0d4f9a697611ffd193ee29baa2962))
- **sdk:** add Python worker helpers ([`a460a06`](https://github.com/jolars/metewand/commit/a460a065e989a807dee0ca7610a4f2eca5c29d84))
- **cli:** add `status` and `explain` commands ([`cc4e043`](https://github.com/jolars/metewand/commit/cc4e043526a3942ea7ad769a2f30f5657e941763))
- **runtime:** add selected execution and resume ([`afa6b2b`](https://github.com/jolars/metewand/commit/afa6b2b39f435e5111eb8ee5fd19b2742eb73873))
- **runtime:** add bounded concurrent execution ([`2f7a145`](https://github.com/jolars/metewand/commit/2f7a145a216c4b3a8a8c98b0c8f2131715ad52da))
- **runtime:** add deterministic block scheduling ([`fda1b30`](https://github.com/jolars/metewand/commit/fda1b30a519702406aecd13f5c3d7184fc9297c0))
- **runtime:** record execution control guarantees ([`f5d2e45`](https://github.com/jolars/metewand/commit/f5d2e457e8ee157f79352609eb9bd4f47938e11d))
- **runtime:** implement worker reuse and warm-ups ([`c865ddc`](https://github.com/jolars/metewand/commit/c865ddcaf26fc027528996e1193358b0eefa50de))
- **runtime:** implement timing envelopes and deadlines ([`140a48d`](https://github.com/jolars/metewand/commit/140a48db81fdfee2d8627b5c44c5bf3c341798cf))
- **runtime:** record phase and resource measurements ([`5c60984`](https://github.com/jolars/metewand/commit/5c60984ad1678a91fae0967d52ac65efaa73460a))
- **runtime:** isolate worker environments ([`21d06c6`](https://github.com/jolars/metewand/commit/21d06c65fb678fe4144a116d8f75bf75c6612938))
- **runtime:** add reusable local process executor ([`2a70bb0`](https://github.com/jolars/metewand/commit/2a70bb0eb67aa38f20a8b21f72002822d33a1427))
- **runtime:** recover interrupted durable attempts ([`039233f`](https://github.com/jolars/metewand/commit/039233fef07cd7c3c675bd671e9ac58c4ad0f1d7))
- **runtime:** publish artifacts transactionally ([`c3c9364`](https://github.com/jolars/metewand/commit/c3c9364d750bc9b86d3776a647cfc1879ece139d))
- **runtime:** add transactional attempt transitions ([`7f166d7`](https://github.com/jolars/metewand/commit/7f166d730c751fce0e4adc1465cd472ae35a47d1))
- **runtime:** initialize durable run stores ([`6e9257c`](https://github.com/jolars/metewand/commit/6e9257c250fb6045c24c30a2c8bc00fdfe4f5937))
- **runtime:** define durable run-store schema ([`4319411`](https://github.com/jolars/metewand/commit/4319411733b984cd4a060cfa00b4ff20848f49e6))
- **runtime:** add optional Datamonger adapter ([`2808dac`](https://github.com/jolars/metewand/commit/2808dac7e49bf8f3971ff280fa3228996f95842f))
- **runtime:** enforce dataset integrity ([`aeb4e77`](https://github.com/jolars/metewand/commit/aeb4e775531bd631ee2e4532f635e942dcdece7e))
- **runtime:** store artifacts and lease dataset views ([`e128679`](https://github.com/jolars/metewand/commit/e1286793dfd8135c95ebdbbd482e9df2edf2c958))
- **runtime:** acquire and materialize pinned datasets ([`d24a8d4`](https://github.com/jolars/metewand/commit/d24a8d45ba0316aa597e8f29167dc416f80ee3a7))
- **runtime:** define source-resolution boundary ([`0cd17a6`](https://github.com/jolars/metewand/commit/0cd17a6f470790dcc824c5e3e511f5de3b8c44e0))
- **cli:** expose `run` with versioned events ([`9915874`](https://github.com/jolars/metewand/commit/9915874b5d068c3f35c139a1bd8f59697ed13f3c))
- **runtime:** finalize observations and attempts ([`b0001a6`](https://github.com/jolars/metewand/commit/b0001a662eec3efaf6b0699fedf015a80720297f))
- validate evaluator verdicts before acceptance ([`990c3c6`](https://github.com/jolars/metewand/commit/990c3c6a6f0084dd2d415f4449ab809cc65ab1ef))
- classify private one-shot run outcomes ([`b116e59`](https://github.com/jolars/metewand/commit/b116e59b0b5938d1cd5e5191df1c9ba62a93e036))
- **runtime:** run fixed dataset with evaluator ([`c8c4d22`](https://github.com/jolars/metewand/commit/c8c4d2224ac3b3336a3144516ff37431a2956bfc))
- **runtime:** add trusted local launch ([`4c9875e`](https://github.com/jolars/metewand/commit/4c9875e272b81bb187101b48eaf401d75d8b7415))
- **runtime:** add raw worker fixtures ([`e49d39e`](https://github.com/jolars/metewand/commit/e49d39ef17e07dd3b7fdad5631cda15c96e16e69))
- **runtime:** validate worker outputs ([`15c054e`](https://github.com/jolars/metewand/commit/15c054e6aef5cb6db837aab2c265a5d868142980))
- **protocol:** add one-shot worker exchanges ([`bde924c`](https://github.com/jolars/metewand/commit/bde924ccb8917d9791504cb69a726a8d2a516621))
- **runtime:** add POSIX worker transport ([`3a59fc0`](https://github.com/jolars/metewand/commit/3a59fc04172aa4224f9bd2897ab75e10d426c9e0))
- **protocol:** add v1 handshake ([`bc67a05`](https://github.com/jolars/metewand/commit/bc67a057807c45ef3c9342666b8aca9c00c9aaf0))
- **protocol:** add adversarial framing ([`f537ed2`](https://github.com/jolars/metewand/commit/f537ed270e4400a2a014a52cef1dbf4f5d06c256))
- **cli:** add machine-readable output ([`2308d04`](https://github.com/jolars/metewand/commit/2308d046159da4e6128c4d46b5da8084977ab13c))
- **cli:** add read-only planning commands ([`82c269e`](https://github.com/jolars/metewand/commit/82c269e6bd4c566b434e4821e9a94a61ee0745f3))
- model capability evidence ([`af8ab59`](https://github.com/jolars/metewand/commit/af8ab59f4c696a0b0d5d066d0820ae63e05b2e41))
- expand logical run plans ([`28ea871`](https://github.com/jolars/metewand/commit/28ea87185653648dac07020c9ee9f0182e9fac36))
- expand parameter configurations ([`20b2c6e`](https://github.com/jolars/metewand/commit/20b2c6e94c7f28f64fa59133b34d297b9699c172))
- add manifest and seed hashing ([`6354f61`](https://github.com/jolars/metewand/commit/6354f61a35e3ab0600436bb92e9c1c6f1d9849f1))
- hash normalized local trees ([`a10ff38`](https://github.com/jolars/metewand/commit/a10ff3852fbcd7ef521217e1f0db4cf559228ee8))
- implement typed content identities ([`b98d642`](https://github.com/jolars/metewand/commit/b98d64272154ab0b0e8a2941f1412f5150a0c356))
- define typed domain records ([`df34c72`](https://github.com/jolars/metewand/commit/df34c726617c9eaaf6720abb93bcfea930bc9fbb))
- validate manifest compatibility ([`13d43b6`](https://github.com/jolars/metewand/commit/13d43b61de2bed59f5e5fbd0b5d05a3baa5dfb2e))
- expand repository source bundles ([`47185ee`](https://github.com/jolars/metewand/commit/47185eecc7bf39dada6494caad15a51f0da43326))
- validate parameter namespaces ([`63a401b`](https://github.com/jolars/metewand/commit/63a401b67ffc3f75818b662d0516aac3b22e3957))
- parse problem contracts ([`ba3c166`](https://github.com/jolars/metewand/commit/ba3c166b25fb7efd11d661851230dd5228e8c651))
- parse version-1 manifests ([`cdd9ea3`](https://github.com/jolars/metewand/commit/cdd9ea3323e97ad4e001c0f40f9a152241c165eb))
- publish versioned public schemas ([`061afdd`](https://github.com/jolars/metewand/commit/061afdd2b0df04f660e64accbb76f79958e14455))
- add schema validation kernel ([`4a9a7b2`](https://github.com/jolars/metewand/commit/4a9a7b266a9ab75b73ddf3b51b60f10033373cd9))
- add canonical JSON kernel ([`78dfa77`](https://github.com/jolars/metewand/commit/78dfa771da2ab6c4299f4445f56791372dbfc471))
- setup package ([`7bbc9c1`](https://github.com/jolars/metewand/commit/7bbc9c1235ec29f5f647176bb98a2eaf3bcb77ec))

### Bug Fixes

- **ci:** stabilize concurrent fresh-sequence test ([`f999adb`](https://github.com/jolars/metewand/commit/f999adbabdb0c45ca4d0e71e2872f74e433fe259))
- **ci:** preserve R site libraries in starter tests ([`ec1d8b9`](https://github.com/jolars/metewand/commit/ec1d8b9b36c3ec4b359c540fa73615504b3dab24))
- retain callback exception diagnostics ([`82dee55`](https://github.com/jolars/metewand/commit/82dee55a1b699be1292a6b27076ac0a8be88da10))
- preserve Python virtual environment context ([`ade6f73`](https://github.com/jolars/metewand/commit/ade6f73016dd3e42357124165577724bfbd53d08))
- **security:** bump undici ([`953cfe4`](https://github.com/jolars/metewand/commit/953cfe4d7c7fdc6cd0e04e55ad170c5af11efcc7))
- bound cleanup after worker process escape ([`c39f464`](https://github.com/jolars/metewand/commit/c39f4649b71178d79e569d5c83369f307630d57f))
- **runtime:** adapt to `sha2` 0.11 ([`0149624`](https://github.com/jolars/metewand/commit/01496248550b71b2c711b947ff605e9394febbd6))

### Performance Improvements

- speed up development test runs ([`726e270`](https://github.com/jolars/metewand/commit/726e270d8f3c3ab78a1851b3136194828e70ae1e))

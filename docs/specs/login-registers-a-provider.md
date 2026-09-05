# login-registers-a-provider

## Goal

`pengepul login --provider <id> --base-url <url> --key <k>` mendaftarkan
provider OpenAI-compatible dan menyimpan kredensialnya dalam satu
perintah, tanpa mengedit `config.yaml` dengan tangan.

Kata operatornya: *"kayaknya agak repot ya kalo harus edit manual,
kenapa ga sekalian pas login aja? nambah --base-url optional"*.

## The shape

```
$ pengepul login --provider openrouter \
    --base-url https://openrouter.ai/api/v1 \
    --key sk-or-v1-...
registered openrouter
saved openrouter account token for key-1a2b3c4d

$ systemctl --user restart pengepul
$ pengepul accounts
```

Tanpa `--base-url`, `login` berperilaku persis seperti sekarang.

## Decisions

- **Mendaftarkan yang baru, menolak yang sudah ada.** `--base-url` untuk
  provider yang sudah terkonfigurasi dengan URL *berbeda* adalah error
  yang menyebut nilai lamanya. Satu salah ketik tidak boleh memindahkan
  trafik sebuah provider yang sedang jalan ke host lain. URL yang
  *sama* diterima — itu bukan konflik, dan mengulang perintah yang sama
  harus aman.
- **`--base-url` menuntut `--key`.** Mendaftar tanpa kredensial
  meninggalkan provider yang terkonfigurasi tapi tak punya akun: sebuah
  keadaan setengah jadi yang tidak menyelesaikan apa pun.
- **Slash di akhir dirapikan.** `base_url` hanya disambung menjadi
  `{base_url}/chat/completions`, jadi `…/v1/` menghasilkan
  `//chat/completions` — 404 di sebagian host, dengan pesan yang tidak
  menunjuk ke penyebabnya.
- **Token dulu, config belakangan.** Kalau penulisan config gagal
  setelah token tersimpan, yang tertinggal adalah token yatim di
  auth-dir: tidak berbahaya, karena providernya memang belum
  terkonfigurasi. Urutan sebaliknya meninggalkan provider terdaftar
  tanpa akun — keadaan yang justru ditolak di atas.

## Non-goals

- **Bukan mengubah provider yang sudah ada.** Tidak ada `--force`, tidak
  ada penimpaan. Mengganti `base-url` tetap lewat editor.
- **Bukan menghapus provider.** Tidak ada `login --remove`; itu verb
  lain, dengan pertanyaannya sendiri soal token yang tertinggal.
- **Tidak menyentuh provider bawaan.** `anthropic` dan `codex` memakai
  OAuth dan URL-nya tetap; `--base-url` di sana adalah error, sama
  seperti `--key` sekarang.
- **Tidak memvalidasi URL lewat jaringan.** `login` tidak menghubungi
  host itu untuk membuktikan ia hidup. Perintah ini menulis konfigurasi;
  request pertama yang membuktikannya.
- **Tidak menyentuh format config.** `providers:` tetap peta
  `id -> {base-url}` dengan `deny_unknown_fields`.
- **Tidak me-restart relay.** Provider dibaca saat start, jadi restart
  tetap urusan operator — dan perintahnya mengatakan itu.

## Acceptance criteria

- AC-1: `login --provider <baru> --base-url <url> --key <k>` menulis
  `providers.<baru>.base-url` ke file config yang sama dengan yang
  dibaca `env.load()`, menyimpan tokennya, lalu mencetak `registered
  <id>` sebelum baris token.
- AC-2: `--base-url` tanpa `--key` gagal, tidak menulis config, dan
  pesannya menyebut bahwa pendaftaran butuh kredensial.
- AC-3: `--base-url` dengan URL berbeda untuk provider yang sudah ada
  gagal, menyebut URL lamanya, dan tidak mengubah file.
- AC-4: `--base-url` dengan URL yang identik (setelah dirapikan)
  berhasil dan menyimpan keynya — mengulang perintah yang sama aman.
- AC-5: Slash di akhir dibuang sebelum ditulis; `https://h/v1/` dan
  `https://h/v1` menghasilkan file yang sama.
- AC-6: `--base-url` bersama provider bawaan (`anthropic`, `codex`)
  gagal, dengan pesan yang menyebut mereka memakai OAuth.
- AC-7: Nama yang ditolak `validate_providers` — nama bergaris miring,
  URL kosong — tetap ditolak lewat jalur ini, dengan pesan yang sama.
  Nama bawaan juga ditolak, tapi lebih awal dan dengan pesan OAuth
  (AC-6): `login` menangkapnya sebelum config disentuh, dan pesan itu
  lebih menjelaskan daripada "is a built-in provider name".
- AC-8: Config yang ditulis memuat kembali tanpa error, dan field lain
  (`api-keys`, `port`, `cloaking`, `timeouts`) tidak berubah nilainya.
- AC-9: Kalau penyimpanan token gagal, config tidak ditulis sama sekali.
- AC-10: `login` tanpa `--base-url` berperilaku identik dengan sekarang,
  untuk provider bawaan maupun yang sudah terkonfigurasi.
- AC-11: Output `registered <id>` mengikuti gaya yang berlaku: satu
  baris teks polos saat piped, dan di panel rich menyatu dengan panel
  `login` yang sudah ada, bukan panel kedua.
- AC-12: `pengepul help login` mendokumentasikan `--base-url` sebagai
  pendaftar provider baru yang menuntut `--key`.

## Verification

```bash
source ~/.cargo/env
cargo test --locked && cargo clippy --all-targets -- -D warnings && cargo fmt --check
TMPDIR=/home/kognos/tmp/a/rather/long/temp/prefix cargo test --locked

# end to end, terhadap config sungguhan (salin dulu, jangan yang hidup):
cp ~/.pengepul/config.yaml /tmp/cfg.yaml
pengepul --config /tmp/cfg.yaml login --provider openrouter \
  --base-url https://openrouter.ai/api/v1/ --key sk-test
grep -A 2 "openrouter" /tmp/cfg.yaml      # tanpa slash di akhir
pengepul --config /tmp/cfg.yaml login --provider openrouter \
  --base-url https://other.host/v1 --key sk-test   # harus gagal
```

Setiap kriteria butuh tes yang gagal ketika perbaikannya dibalik.

## Revisions

- **Pendaftaran menulis ulang file config.** `register_provider` mem-parse
  `RawConfig`, menyisipkan satu entri, lalu menulis ulang seluruh file
  lewat `serde_yaml`. Nilainya utuh — AC-8 tetap berlaku — tapi komentar
  hilang, urutan kunci mengikuti urutan struct, dan bagian yang selama ini
  memakai default (`cloaking`, `timeouts`, `stats`, `debug`, `body-limit`)
  jadi tertulis eksplisit dengan nilai default hari ini. README mengajak
  operator mengedit file ini dengan tangan, jadi ini terlihat olehnya, dan
  README sekarang mengatakannya.
- **Konflik ditolak sebelum kredensial ditulis.** Urutan token-dulu
  beralasan bahwa token yatim tidak berbahaya karena providernya belum
  terkonfigurasi. Di jalur AC-3 providernya justru sudah terkonfigurasi
  dan hidup: satu salah ketik id menaruh key asing ke dalam pool provider
  itu, yang akan ikut rotasi setelah reload dan menjawab 401. Pengecekan
  konflik karena itu naik ke depan `save_token`, memakai `config.providers`
  yang sudah dimuat. Ditemukan oleh judge; direproduksi sebelum diperbaiki.
- **Id validasi jadi allowlist, bukan denylist.** Ronde 2 menolak `/`
  saja. Menguji binary hasil build dengan 18 bentuk id menunjukkan dua
  belas yang lain tetap menulis kredensial — `..` ke luar auth-dir, id
  kosong ke akarnya, `\` dan spasi dan baris baru masing-masing ke
  direktorinya sendiri — dan semuanya **terdaftar** di config, jadi ikut
  dimuat setiap start. Suite hijau selama itu. Aturannya sekarang:
  huruf, angka, `.`, `-`, `_`, dengan `.` dan `..` ditolak by name.
  Ditemukan dengan menjalankan, bukan dengan membaca.

## Not yet specified

- **Tabrakan id yang hanya beda huruf besar-kecil.** `providers:` adalah
  peta case-sensitive dan `storage_dir()` mengembalikan id apa adanya,
  jadi `GROQ` dan `groq` adalah dua Provider dengan dua direktori — benar
  di Linux. Di filesystem case-insensitive (default macOS) keduanya
  berbagi satu direktori, sehingga tiap pool memuat kunci milik yang
  lain. Ini milik model Provider yang sudah ada, bukan verb ini:
  mengedit config dengan tangan sudah bisa melakukannya sejak dulu.
  Dicatat, tidak diperbaiki di sini.

# launch-runs-a-harness

## Goal

`pengepul launch claude` dan `pengepul launch pi` menjalankan harness
coding di atas relay, tanpa meninggalkan apa pun setelah prosesnya
selesai. `claude` dan `pi` yang dijalankan biasa tetap berperilaku persis
seperti sebelumnya.

Kata operatornya: *"gua mau claude pi tetap berfungsi kek biasa, tapi
kalo pake pengepul launch pi/claude dia pake model dari pengepul"*.

Hari ini jalan menuju ke sana ada di README, sebagai config yang disalin
dengan tangan per klien — biaya yang memang sengaja ditaruh di operator
(ADR-0007). `launch` tidak memindahkan biaya itu ke server: rutenya tetap
generik, kliennya tetap yang menyesuaikan diri. Yang berubah, penyesuaian
itu hidup selama satu proses saja.

## The shape

```
$ pengepul launch claude --model gpt-5.4
$ pengepul launch pi --model anthropic/claude-opus-5
$ pengepul launch claude -- --resume
```

Tanpa `--model`, relay ditanya modelnya dan operator memilih:

```
$ pengepul launch pi

  models (78)

    1  anthropic/claude-fable-5    1.0M ctx  $10.00/$50.00
    ...
   20  commandcode/gpt-5.6-sol     1.0M ctx  $4.00/$20.00
       58 more

  pick a number, or type to filter: glm

  models — "glm" (6 of 78)

    1  commandcode/z-ai/glm-5.3-flash    1.0M ctx
    ...

  pick a number, type to filter, or enter to clear it: flash

  models — "glm flash" (1 of 78)

    1  commandcode/z-ai/glm-5.3-flash    1.0M ctx

  pick a number, type to filter, or enter to clear it: 1
```

Berhasil berarti tidak mencetak apa-apa: prosesnya berubah menjadi
harness itu sendiri, seperti `serve` berubah menjadi relay. Yang tercetak
hanya penolakan.

## Decisions

- **Variabel lingkungan, bukan file.** Harness yang masuk hanya yang bisa
  dialihkan per-proses. Tidak ada file operator yang ditulis, tidak ada
  cadangan yang perlu dibuat, dan `claude`/`pi` tanpa `launch` tetap
  menemukan akun dan modelnya sendiri. Ini juga yang memilih daftar
  harness-nya: openclaw dan hermes tidak punya variabel per-provider —
  diperiksa terhadap binernya — jadi keduanya berarti menulis
  `openclaw.json` dan `config.yaml` milik operator, janji yang lain.
- **`exec`, bukan spawn.** Terminal, sinyal dan exit code jadi milik
  harness sejak detik pertama. pengepul tidak jadi induk yang harus
  meneruskan Ctrl-C dan menerjemahkan status keluar.
- **Relay ditanya dulu.** `launch` memanggil `/health` sebelum menyusun
  rencananya. Relay yang mati adalah penolakan perintah ini; diserahkan
  ke harness, fakta yang sama muncul sebagai connection error di dalam
  TUI, beberapa layar dari apa pun yang menyebut pengepul.
- **pi menuntut `--model`, claude tidak.** Diukur, bukan dibaca:
  `pi --provider openrouter` menjawab dari provider di settings-nya
  sendiri dan tidak berkata apa-apa — pi baru mengikat `--provider`
  kalau ada model bersamanya. Tanpa model, verb ini akan menjalankan pi
  yang tampak terarah ke relay padahal tidak. Claude Code membawa daftar
  modelnya sendiri, dan id itu memang milik anthropic, jadi di sana
  `--model` adalah pilihan. Di terminal, picker yang mengisi keduanya;
  tuntutan pi baru terasa kalau pickernya ditolak atau outputnya di-pipe.
- **Satu pertanyaan, dua jawaban.** Katalognya 78 model, jadi daftar
  bernomor saja tidak cukup. Prompt yang sama menerima keduanya: angka
  memilih baris, teks lain menyaring. Tidak ada mode kedua, tidak ada
  pertanyaan kedua, dan tidak ada dependensi baru — proyek ini belum
  pernah menambah satu pun untuk tampilan.
- **Menyaring itu mempersempit, bukan mengganti.** Setelah daftar berkepala
  `"glm" (6 of 78)`, mengetik `flash` jelas berarti yang flash di antara
  keenam itu. Enter mengosongkan seluruh saringan, dan pertanyaannya
  mengatakan itu.
- **Yang tidak bisa dilayani tidak ditawarkan.** Daftar untuk claude hanya
  berisi model yang bisa menjawab Messages, jadi 501 itu tidak bisa
  dipilih sama sekali — aturan yang sama dengan `--model` eksplisit,
  dipakai lebih awal. Pool tanpa akun juga tidak muncul: `/v1/models`
  memang tidak mengiklankannya.
- **Piped berarti tidak bertanya.** Menu di atas pipe akan memakan satu
  baris milik skrip orang. `stdout_is_tty` sudah jadi seam sejak
  `Style::from_tty`, dan verb ini memakainya lagi: tanpa terminal,
  perilakunya persis seperti sebelum picker ada.
- **`--model` menggerakkan semua tier.** Operator menyebut satu model;
  `/model sonnet` yang diam-diam pergi ke tempat lain adalah kesunyian
  yang sama yang verb ini hapus. `ANTHROPIC_MODEL` dan tiga tier plus
  model subagent semuanya menunjuk ke sana.
- **`ANTHROPIC_API_KEY` dikosongkan, bukan dibiarkan.** Key yang sudah
  ada di lingkungan operator mengalahkan `ANTHROPIC_AUTH_TOKEN`, dan
  mengirim harness ke api.anthropic.com dengan meteran per-token — satu
  hal yang justru dihindari verb ini.
- **Model provider terkonfigurasi ditolak untuk claude.** Claude Code
  bicara dialek Messages, dan endpoint OpenAI-compatible menjawab 501
  untuk itu. Id model sudah menyebut providernya, jadi penolakannya lokal
  dan datang sebelum harness menyala, bukan sebagai 501 di prompt
  pertama.
- **Ekstensi pi adalah prasyarat, bukan urusan `launch`.** Provider
  `pengepul` di pi didaftarkan `@pwguler/pi-pengepul-provider`. Menaruh
  `-e npm:@pwguler/pi-pengepul-provider` di baris exec membuat verb ini
  berdiri sendiri, tapi diukur: +2 detik di **setiap** peluncuran, untuk
  menutup satu instalasi sekali seumur hidup. Tanpa itu pi menolak dengan
  `Unknown provider "pengepul"` — nyaring dan menunjuk. `pengepul help
  launch` dan README menyebut perintah pemasangannya.

## Non-goals

- **Bukan openclaw dan hermes.** Keduanya hanya bisa diarahkan lewat file
  config mereka. Itu janji lain — cadangan, penggabungan dengan isi yang
  sudah ada, komentar yang hilang — dan README tetap menjelaskan caranya
  dengan tangan.
- **Bukan picker bertombol panah.** Tidak ada raw mode, tidak ada crate
  TUI baru. Satu daftar, satu pertanyaan, satu baris jawaban — jalan di
  ssh, di tmux, dan di terminal apa pun yang bisa membaca satu baris.
- **Bukan mengingat pilihan terakhir.** Itu berarti menulis sesuatu ke
  disk, dan verb ini tidak meninggalkan apa-apa.
- **Tidak memvalidasi model lewat jaringan.** `launch` tidak bertanya ke
  `/v1/models` apakah id itu ada. Yang diperiksa hanya dialeknya, dan itu
  bisa dijawab dari config. Request pertama yang membuktikan sisanya.
- **Tidak menyalakan relay.** Relay yang mati adalah penolakan yang
  menyebut perintahnya, bukan sesuatu yang `launch` benahi sendiri.
- **Tidak memasang harness.** Biner yang tidak ada jadi pesan yang
  menyebut baris pemasangannya, bukan pemasangan.
- **Tidak mencetak panel.** Harness akan menimpa layar dalam sepersekian
  detik; `serve` juga tidak mencetak apa-apa.

## Acceptance criteria

- AC-1: `launch claude` menyerahkan proses ke `claude` dengan
  `ANTHROPIC_BASE_URL` dan `ANTHROPIC_AUTH_TOKEN` dari config yang sama
  yang dibaca verb lain, dan `ANTHROPIC_API_KEY` kosong.
- AC-2: `launch claude` tanpa `--model` tidak menyetel satu pun variabel
  model; harness memakai daftarnya sendiri.
- AC-3: `launch claude --model <id>` menyetel `ANTHROPIC_MODEL`, ketiga
  `ANTHROPIC_DEFAULT_*_MODEL`, dan `CLAUDE_CODE_SUBAGENT_MODEL` ke id
  yang sama.
- AC-4: `launch claude --model <p>/<m>` dengan `<p>` sebuah entri
  `providers:` gagal, menyebut nama providernya dan Chat Completions, dan
  tidak menjalankan apa pun. Prefiks `anthropic/` dan `codex/` lolos:
  keduanya bawaan, bukan entri config.
- AC-5: `launch pi --model <id>` menjalankan `pi --provider pengepul
  --model <id>` dengan `PENGEPUL_BASE_URL` dan `PENGEPUL_API_KEY` di
  lingkungannya.
- AC-6: `launch pi` tanpa `--model` gagal, pesannya menyebut `--model`,
  dan tidak menjalankan apa pun.
- AC-7: Argumen setelah `--` sampai ke harness apa adanya, di belakang
  argumen yang disusun `launch` sendiri.
- AC-8: Relay yang tidak menjawab menggagalkan perintah sebelum rencana
  tersusun, dengan pesan yang menyebut URL-nya dan cara menyalakannya.
- AC-9: `--config` diikuti seperti verb lain; base URL dan key datang
  dari file yang ditunjuk.
- AC-10: Harness yang tidak dikenal ditolak saat parsing argumen, dengan
  pesan yang menyebut nilai yang ada.
- AC-11: Berhasil berarti stdout kosong.
- AC-12: `pengepul help launch` menyebut kedua harness, `--model`, dan
  separator `--`.
- AC-13: Tanpa `--model` dan di terminal, `launch` membaca
  `/v1/models` dan menawarkan isinya; angka memilih satu baris.
- AC-14: Daftar untuk claude tidak memuat model dari entri `providers:`;
  daftar untuk pi memuat semuanya.
- AC-15: Jawaban yang bukan nomor baris jadi saringan, dan saringan
  berikutnya mempersempit hasil sebelumnya, bukan menggantikannya.
- AC-16: Jawaban kosong mengosongkan saringan kalau ada, dan menolak
  memilih kalau tidak ada. Habisnya input menolak memilih.
- AC-17: Daftar berhenti di 20 baris dan menyebut berapa yang disimpan.
- AC-18: Dengan output di-pipe, tidak ada pertanyaan dan tidak ada
  permintaan `/v1/models`; perilakunya sama seperti sebelum picker ada.
- AC-19: `--model` yang eksplisit melewati picker sepenuhnya.
- AC-20: `launch` sendiri tidak menulis file apa pun: tidak ada config
  harness yang berubah karena satu peluncuran. Apa yang ditulis harness
  setelah mengambil alih proses adalah urusannya sendiri.

## Verification

```bash
source ~/.cargo/env
cargo test --locked && cargo clippy --all-targets -- -D warnings && cargo fmt --check

# end to end, terhadap relay yang hidup
pengepul status | grep '^requests'          # catat
pengepul launch claude -- -p "reply with exactly: pong"
pengepul status | grep '^requests'          # naik
sha256sum ~/.pi/agent/settings.json         # catat
pengepul launch pi --model commandcode/z-ai/glm-5.3-flash -- -p "reply with exactly: pong"
sha256sum ~/.pi/agent/settings.json         # sama

# dan yang membuktikan janjinya: claude biasa tidak lewat relay
pengepul status | grep '^requests'          # catat
claude -p "reply with exactly: pong"
pengepul status | grep '^requests'          # tidak berubah

# penolakan
pengepul launch pi                          # menuntut --model
pengepul launch claude --model commandcode/z-ai/glm-5.3-flash
pengepul --config /tmp/dead.yaml launch claude   # port yang tidak ada
```

Setiap kriteria butuh tes yang gagal ketika perbaikannya dibalik.

## Revisions

- **Menyaring dua kali mengganti, bukan mempersempit.** Versi pertama
  menimpa saringan dengan jawaban terakhir. Dijalankan di pty sungguhan:
  `glm` menyisakan 6 baris, lalu `flash` melebar lagi jadi 14 — bukan
  satu model glm yang flash. Sekarang jawaban ditambahkan ke saringan.
  Ditemukan dengan menjalankan, bukan dengan membaca.

## Not yet specified

- **codex.** Bisa diarahkan per-proses juga, lewat `-c
  model_providers.pengepul.*` di baris exec, jadi ia masuk aturan
  "variabel lingkungan, bukan file" tanpa perubahan bentuk. Ditunda
  karena binernya tidak ada di mesin ini untuk dibuktikan.
- **Harness yang butuh file config.** openclaw dan hermes menunggu
  keputusan soal cadangan dan penggabungan — pertanyaan yang sama yang
  `register_provider` sudah bayar mahal untuk `config.yaml`.
- **Pool kosong di balik `--model`.** `launch claude --model gpt-5.4` ke
  relay tanpa akun codex membuat relay menjawab 503 — rutenya benar,
  poolnya yang kosong — dan Claude Code mengulanginya dengan backoff, jadi
  yang dilihat operator adalah perintah yang menggantung, bukan penolakan.
  pi tidak begitu: ia mencetak pesan relay apa adanya lalu berhenti.
  `launch` tidak memeriksa pool dari model yang disebut: itu butuh
  panggilan admin lagi dan sebuah aturan soal pool yang kosong hanya
  sementara.
- **`--model` adalah pola di pi, id di claude.** `launch claude` mengirim
  id itu apa adanya dan routing pengepul yang memutuskan. `launch pi`
  menyerahkannya ke pi, yang mencocokkannya sebagai *pola* terhadap
  katalog yang relay iklankan — jadi `--model gpt-5.4` bisa mendarat di
  provider terkonfigurasi, bukan di codex. Diukur: satu 403
  `MODEL_NOT_IN_PLAN` dari commandcode, bukan 503 dari codex. Yang
  menutupnya adalah menyebut prefiksnya (`codex/gpt-5.4`), bentuk yang
  memang diiklankan `/v1/models`. Belum diputuskan apakah `launch pi`
  harus menuntut prefiks itu.
- **Ekstensi pi yang belum terpasang.** `launch pi` menyerahkan
  penolakannya ke pi. Kalau suatu hari pesan pi itu berubah, operator
  kehilangan petunjuknya dan `launch` tidak tahu.

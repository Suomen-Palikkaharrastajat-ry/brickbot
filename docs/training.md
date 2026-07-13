# Ambient Assistant: Training & Data Gathering

This document outlines how the Brickbot Ambient Conversational Assistant generates training data and how users interact with it.

## 1. How It Works (The User Experience)

Because Brickbot is an **ambient** assistant, there are no specific slash commands required to "use" it. The bot passively listens to channel messages in the background.

If a user types a message in any monitored channel containing specific keywords or patterns (e.g., in English or Finnish), the bot automatically detects the topic:
- *"I got set 42083 for my birthday!"*
- *"Mistä löydän osan 3001?"*
- *"When is the next Lego miitti?"*

The event handler (`src/main.rs`) runs `detect_topic` on every incoming message. If the confidence score is `Medium` or `High` (and the channel isn't on cooldown), the bot responds with an ephemeral message containing interactive Discord Buttons offering assistance.

## 2. Phase 1: Data Gathering (Current State)

Currently, the bot uses a **Phase 1 Aho-Corasick Weighted Engine** (`src/ambient.rs`) to detect topics using predefined keywords and regex patterns. 

By default, Phase 1 operates entirely in-memory and **does not store message data**. 

### Implementing the Data Logger
To gather data for future machine learning models, the bot needs to be configured to log detected topics. 

When implemented, the flow should look like this:
1. `detect_topic()` identifies a topic with `Medium` or `High` confidence.
2. Before responding, the bot executes an async SQLite insert into an `ambient_logs` table.
3. The table stores: `timestamp`, `original_message_content`, and `detected_topic`.

Over time, this table passively grows into a large, highly-accurate dataset of real-world Discord conversations mapped to their correct topics.

## 3. Phase 2: Training a FastText Classifier (Future)

Generating synthetic data with an LLM produces clean, grammatically correct sentences. However, real Discord users type with slang, typos, and fragmented sentences. 

Instead of synthetic data, we use the `ambient_logs` table generated during Phase 1. 

Once the logs have gathered a few thousand rows:
1. **Export & Review:** Export the SQLite table and quickly review it to remove any false positive detections.
2. **Train FastText:** Use the dataset to train an offline FastText model (`fasttext-pure-rs`).
3. **Upgrade Engine:** Replace the Aho-Corasick rules-based engine with the FastText `.ftz` model.

This approach guarantees the Phase 2 bot is trained specifically on the actual vocabulary, slang, and typos used in your unique LEGO communities.

## 4. Using the Trainer CLI Tool

To assist with the transition from Phase 1 to Phase 2, a companion CLI tool (`trainer`) is included. It allows you to manually review the gathered logs and export them into a FastText-compatible training format.

### Running the Tool

You must run the tool within the Nix `devenv` shell to ensure all database and compilation dependencies (e.g., SQLite, MUSL) are correctly loaded.

```bash
devenv shell -- cargo run --bin trainer -- --help
```

### Reviewing Logs

To interactively review all pending detections gathered by the bot:

```bash
devenv shell -- cargo run --bin trainer -- review
```

For each logged message, you will see the detected topic and be prompted for an action:
- `a` (Accept): Confirm the bot's topic detection is correct.
- `r` (Reject): Mark the detection as a false positive (it will be excluded from the dataset).
- `c` (Correct): Manually type in the correct topic label if the bot guessed wrong.
- `s` (Skip): Leave the log as `pending` to review later.
- `q` (Quit): Exit the review session and save progress.

### Exporting the Dataset

Once you have reviewed a sufficient number of logs, you can export them to a text file formatted for FastText training:

```bash
devenv shell -- cargo run --bin trainer -- export --output train.txt
```

This generates a file (`train.txt` by default) where each line represents a reviewed interaction:

```text
__label__LegoSet I bought a new set yesterday!
__label__Events When is the next Lego miitti?
```

### Training the FastText Model

With your curated `train.txt` file ready, you can now use the official FastText tooling to train the classification model:

```bash
fasttext supervised -input train.txt -output model
```

Once you have the resulting `.ftz` or `.bin` model, it can be integrated with `fasttext-pure-rs` inside the bot to replace the Aho-Corasick engine.

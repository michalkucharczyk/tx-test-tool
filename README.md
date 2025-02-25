## Transaction Testing Tool (`txtt`)

`txtt` is a library and a command-line interface (CLI) designed for testing a transaction pool on substrate-based chains. It allows developers and testers to simulate various transaction scenarios monitor blocks, and inspect events for every transaction. `txtt` is intended to be a main testing tool for transaction pool.

### Main features:
- single-account scenarios: send single or multiple transactions from a specific account,
- multi-account scenarios: send transactions from a range of accounts,
- automated nonce fetching: automatically fetch and manage nonce for transactions, simplifying testing across multiple accounts,
- submit or submit_and_watch: support for watched and non-watched transaction sending,
- block monitoring: monitor blocks for transaction finalization,
- per transaction execution log: all transaction events are stored in journal file for latter inspection, allowing also to track duration between execution events,
- test execution abstracted from the chain type, theoretically allowing to re-use execution scenarios across multiple chains,
- tracking *dropped* or *invalid* transactions, and resubmitting them when appropriate,

### Installation:

To install `txtt`, clone the repository and build the project using Cargo:
```
git clone git@github.com:michalkucharczyk/tx-test-tool.git
cd tx-test-tool
cargo build --release
```

The binary will be available in the `target/release/` directory.

### Accounts
`txtt` uses accounts abstraction on CLI input. It can be account number or `Keyring` identifier (`alice`, `bob`, `alith`, `balthazar`, etc...).
Account number is used for tool-specific derivation, so make sure to include endowed accounts into the chain-spec of test network. `txtt` provides a command to generate `json` file containing `balanaces` pallet compatible entries..

### Usage

The following examples demonstrate how to use `txtt` for various transaction testing scenarios.

#### Basic Commands

##### Sending a One-shot Transaction

Send a single transaction from an account with a specified nonce:
```
txtt tx one-shot --account alice --nonce 1
```

This command sends one transaction from the account `alice` with a nonce of 1 to the default Substrate chain using the default WebSocket endpoint (`ws://127.0.0.1:9933`) and listens for transaction status events.

##### Sending Transactions with Automatic Nonce

Automatically fetch the nonce for the account before sending the transaction:
```
txtt tx one-shot --account bob
```

Here, `txtt` fetches the nonce for the account `bob` from the blockchain network and sends the transaction.

##### Sending Multiple Transactions from a Single Account

Send a range of transactions from a single account:

```
txtt tx from-single-account --account alice --from 10 --count 15
```

This command sends transactions with nonces ranging from 10 to 25 from the account `alice`.

##### Sending Multiple Transactions from Multiple Accounts

Send transactions from multiple accounts, each with an automatically fetched nonce:

```
txtt tx from-many-accounts --start-id 100 --last-id 200 --count 10
```

This sends 10 transactions for every account with IDs ranging from 100 to 200, fetching the nonce for each account automatically.

##### *Non-watched* vs. *watched* transaction


`txtt` by default sends *watched* transactions, meaning that every transaction will be sent using `submit_and_watch` and every status reported by node will be tracked (and stored in execution log):
```
txtt tx from-single-account --account alice
```

`txtt` can also send *non-watched* transaction, without monitoring their finalization status (think of *fire-and-forget* mode):
```
txtt tx --unwatched from-single-account --account alice --count 10
```

The *non-watched* transactions can also be monitored for finalization using `--block-monitor` parameter:
```
txtt tx --unwatched --block-monitor from-single-account --acount alice --count 10
```



#### Helpers

##### Block Monitoring

Monitor blocks on the blockchain to track number of extrinsics (useful for manual testing):

```
txtt block-monitor --ws ws://127.0.0.1:9933
```

##### Log Management

Every execution stores a journal containing execution events for every transaction. Default name can be overwritten with `--log-file` parameter.

```
txtt tx --log-file custom_log.json from-many-accounts --count 5
```

A log file can be later loaded to display graphs or inspect errors:

```
txtt load-log custom_log.json --show-graphs
txtt load-log custom_log.json --show-errors
```

##### Nonce Checking

Intended to be quick util to check the current nonce for a specific account:

```
txtt check-nonce --account alice
```


#### Network Configuration

You can easily switch between different blockchain networks and WebSocket endpoints using the `--chain` and `--ws` options.

##### Switching Chains:

To target an Ethereum-based chain:
```
txtt tx --chain eth one-shot --account alice --nonce 1
```
##### Changing WebSocket Endpoint:

To use a different WebSocket endpoint:
```
txtt tx --ws ws://another-node:1234 one-shot --account alice
```


### Contributing

Contributions to `txtt` are welcome! Feel free to open issues, submit pull requests, or suggest new features.

### License

`txtt` is open-source and licensed under either of Apache License, Version 2.0 or MIT license.


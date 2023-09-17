use anyhow::{anyhow, Result};
use ethers::{
    abi::{self, parse_abi},
    prelude::*,
    providers::Middleware,
    types::{
        transaction::eip2930::AccessList, BlockId, BlockNumber, Eip1559TransactionRequest,
        NameOrAddress, H160, U256,
    },
};
use gas_oracle::middleware;
use log::info;
use revm::primitives::bytes::Bytes;
use revm::{
    db::{CacheDB, EmptyDB, EthersDB, InMemoryDB},
    primitives::Bytecode,
    primitives::{
        keccak256, AccountInfo, ExecutionResult, Log, Output, TransactTo, TxEnv, B160,
        U256 as rU256,
    },
    Database, EVM,
};
use std::{str::FromStr, sync::Arc};

use crate::constants::SIMULATOR_CODE;

#[derive(Debug, Clone)]
pub struct TxResult {
    pub output: Bytes,
    pub logs: Option<Vec<Log>>,
    pub gas_used: u64,
    pub gas_refunded: u64,
}

pub fn create_evm_instance() -> EVM<InMemoryDB> {
    let db = CacheDB::new(EmptyDB::default());
    let mut evm = EVM::new();
    evm.database(db);
    evm
}

pub fn evm_env_setup(evm: &mut EVM<InMemoryDB>) {
    // overriding some default env values to make it more efficient for testing
    evm.env.cfg.limit_contract_code_size = Some(0x1000000);
    evm.env.cfg.disable_block_gas_limit = true;
    evm.env.cfg.disable_base_fee = true;
}

//We’ll try calling the ERC-20 function “balanceOf” to check how much of the token balance our simulated user holds
pub fn get_token_balance(evm: &mut EVM<InMemoryDB>, token: H160, account: H160) -> Result<U256> {
    //First, we create the BaseContract instance from a simple ERC-20 ABI that contains a single function definition:
    let erc20_abi = BaseContract::from(parse_abi(&[
        "function balanceOf(address) external view returns (uint256)",
    ])?);

    //With this, we encode the function call to “balanceOf”, which will result in the below, if we try printing out “calldata”:
    //Bytes(0x70a08231000000000000000000000000e2b5a9c1e325511a227ef527af38c3a7b65afa1d)
    let calldata = erc20_abi.encode("balanceOf", account)?;

    //We, then, have to setup the transaction variables before we can send the call:
    //caller: who is calling the function (from)
    evm.env.tx.caller = account.into();
    //transact_to: what are we calling (to)
    evm.env.tx.transact_to = TransactTo::Call(token.into());
    //data: the input data of our transaction (input)
    evm.env.tx.data = calldata.0;

    let result = match evm.transact_ref() {
        Ok(result) => result,
        Err(e) => return Err(anyhow!("EVM call failed: {e:?}")),
    };

    let tx_result = match result.result {
        ExecutionResult::Success {
            gas_used,
            gas_refunded,
            output,
            logs,
            ..
        } => match output {
            Output::Call(o) => TxResult {
                output: o,
                logs: Some(logs),
                gas_used,
                gas_refunded,
            },
            Output::Create(o, _) => TxResult {
                output: o,
                logs: Some(logs),
                gas_used,
                gas_refunded,
            },
        },
        ExecutionResult::Revert { gas_used, output } => {
            return Err(anyhow!(
                "EVM REVERT: {:?} / Gas used: {:?}",
                output,
                gas_used
            ))
        }
        ExecutionResult::Halt {
            reason, gas_used, ..
        } => return Err(anyhow!("EVM HALT: {:?} / Gas used: {:?}", reason, gas_used)),
    };
    let decoded_output = erc20_abi.decode_output("balanceOf", tx_result.output)?;
    Ok(decoded_output)
}

pub async fn revm_contract_deploy_and_tracing<M: Middleware + 'static>(
    evm: &mut EVM<InMemoryDB>,
    provider: Arc<M>,
    token: H160,
    account: H160,
) -> Result<i32> {
    //deploy contract to EVM
    let block = provider
        .get_block(BlockNumber::Latest)
        .await?
        .ok_or(anyhow!("failed to retrieve block"))?;

    let mut ethersdb = EthersDB::new(provider.clone(), Some(block.number.unwrap().into())).unwrap();

    let token_acc_info = ethersdb.basic(token.into()).unwrap().unwrap();

    evm.db
        .as_mut()
        .unwrap()
        .insert_account_info(token.into(), token_acc_info);

    let erc20_abi = BaseContract::from(parse_abi(&[
        "function balanceOf(address) external view returns= (uint256)",
    ])?);

    let calldata = erc20_abi.encode("balanceOf", account)?;

    evm.env.tx.caller = account.into();
    evm.env.tx.transact_to = TransactTo::Call(token.into());
    evm.env.tx.data = calldata.0.clone();

    let result = match evm.transact_ref() {
        Ok(result) => result,
        Err(e) => return Err(anyhow!("EVM call failed: {e:?}")),
    };
    let token_b160: B160 = token.into();
    let token_acc = result.state.get(&token_b160).unwrap();
    let token_touched_storage = token_acc.storage.clone();
    info!("touched storage slots: {:?}", token_touched_storage);

    for i in 0..20 {
        let slot = keccak256(&abi::encode(&[
            abi::Token::Address(account.into()),
            abi::Token::Uint(U256::from(i)),
        ]));
        let slot: rU256 = U256::from(slot).into();
        match token_touched_storage.get(&slot) {
            Some(_) => {
                info!("Balance storage slot: {:?} ({:?})", i, slot);
                return Ok(i);
            }
            None => {}
        }
    }

    Ok(0)
}

pub fn get_tx_result(result: ExecutionResult) -> Result<TxResult> {
    let output = match result {
        ExecutionResult::Success {
            gas_used,
            gas_refunded,
            output,
            logs,
            ..
        } => match output {
            Output::Call(o) => TxResult {
                output: o,
                logs: Some(logs),
                gas_used,
                gas_refunded,
            },
            Output::Create(o, _) => TxResult {
                output: o,
                logs: Some(logs),
                gas_used,
                gas_refunded,
            },
        },
        ExecutionResult::Revert { gas_used, output } => {
            return Err(anyhow!(
                "EVM REVERT: {:?} / Gas used: {:?}",
                output,
                gas_used
            ))
        }
        ExecutionResult::Halt { reason, .. } => return Err(anyhow!("EVM HALT: {:?}", reason)),
    };

    Ok(output)
}

pub async fn revm_v2_simulate_swap<M: Middleware + 'static>(
    evm: &mut EVM<InMemoryDB>,
    provider: Arc<M>,
    account: H160,
    factory: H160,
    target_pair: H160,
    input_token: H160,
    output_token: H160,
    input_balance_slot: i32,
    output_balance_slot: i32,
    input_token_implementation: Option<H160>,
    output_token_implementation: Option<H160>,
) -> Result<(U256, U256)> {
    let block = provider
        .get_block(BlockNumber::Latest)
        .await?
        .ok_or(anyhow!("failed to retrieve block"))?;

    //Our EthersDB will make calls to the latest block data.
    let mut ethersdb = EthersDB::new(provider.clone(), Some(block.number.unwrap().into())).unwrap();

    let db = evm.db.as_mut().unwrap();

    let ten_eth = rU256::from(10)
        .checked_mul(rU256::from(10).pow(rU256::from(18)))
        .unwrap();

    let user_acc_info = AccountInfo::new(ten_eth, 0, Bytecode::default());
    db.insert_account_info(account.into(), user_acc_info);
    println!("Inserted 10 ETH to user ----");

    //deploy simulator contract
    let simulator_address = H160::from_str("0xF2d01Ee818509a9540d8324a5bA52329af27D19E").unwrap();
    let simulator_acc_info = AccountInfo::new(
        rU256::ZERO,
        0,
        Bytecode::new_raw((*SIMULATOR_CODE.0).into()),
    );

    db.insert_account_info(simulator_address.into(), simulator_acc_info);
    println!("Inserted simulator contract ----");

    // Deploy necessary contracts to simulate uniswap v2 swap
    let input_token_addres = match input_token_implementation {
        Some(implementation) => implementation,
        None => input_token,
    };
    let output_token_address = match output_token_implementation {
        Some(implementation) => implementation,
        None => output_token,
    };
    let input_token_acc_info = ethersdb.basic(input_token_addres.into()).unwrap().unwrap();
    let output_token_acc_info = ethersdb
        .basic(output_token_address.into())
        .unwrap()
        .unwrap();
    let factory_acc_info = ethersdb.basic(factory.into()).unwrap().unwrap();

    db.insert_account_info(input_token.into(), input_token_acc_info);
    db.insert_account_info(output_token.into(), output_token_acc_info);
    db.insert_account_info(factory.into(), factory_acc_info);
    println!("Inserted input, output and factory account info ----");

    //Deploy pair contract using factory
    let factory_abi = BaseContract::from(parse_abi(&[
        "function createPair(address,address) external returns (address)",
    ])?);
    let calldata = factory_abi.encode("createPair", (input_token, output_token))?;

    let gas_price = rU256::from(100)
        .checked_mul(rU256::from(10).pow(rU256::from(9)))
        .unwrap();

    //create a pair contract using the factory contract
    let create_pair_tx = TxEnv {
        caller: account.into(),
        gas_limit: 5000000,
        gas_price: gas_price,
        gas_priority_fee: None,
        transact_to: TransactTo::Call(factory.into()),
        value: rU256::ZERO,
        data: calldata.0,
        chain_id: None,
        nonce: None,
        access_list: Default::default(),
    };
    evm.env.tx = create_pair_tx;

    let result = match evm.transact_commit() {
        Ok(result) => result,
        Err(e) => return Err(anyhow!("EVM call failed{:?}", e)),
    };
    println!("Created a contract using the factory ----");

    let result = get_tx_result(result)?;
    let pair_address: H160 = factory_abi.decode_output("createPair", result.output)?;
    info!("Pair created: {:?}", pair_address);

    let pair_created_log = &result.logs.unwrap()[0];
    let token0: B160 = pair_created_log.topics[1].into();
    let token1: B160 = pair_created_log.topics[2].into();
    info!("Token 0: {:?} Token 1: {:?}", token0, token1);

    //Check if the target_pair is equal to the pair created address
    assert_eq!(target_pair, pair_address);

    // There're no reserves in the pool, so we inject the reserves that we retrieve with ethersdb
    // The storage slot of reserves is: 8
    let db = evm.db.as_mut().unwrap();
    let reserves_slot = rU256::from(8);
    let original_reserves = ethersdb
        .storage(pair_address.into(), reserves_slot)
        .unwrap();
    db.insert_account_storage(pair_address.into(), reserves_slot, original_reserves)?;

    // Check that the reserves are set correctly
    let pair_abi = BaseContract::from(parse_abi(&[
        "function getReserves() external view returns (uint112,uint112,uint32)",
    ])?);
    let calldata = pair_abi.encode("getReserves", ())?;
    let get_reserves_tx = TxEnv {
        caller: account.into(),
        gas_limit: 5000000,
        gas_price: gas_price,
        gas_priority_fee: None,
        transact_to: TransactTo::Call(target_pair.into()),
        value: rU256::ZERO,
        data: calldata.0,
        chain_id: None,
        nonce: None,
        access_list: Default::default(),
    };
    evm.env.tx = get_reserves_tx;

    let result = match evm.transact_ref() {
        Ok(result) => result,
        Err(e) => return Err(anyhow!("EVM call failed: {:?}", e)),
    };
    let result = get_tx_result(result.result)?;
    let reserves: (U256, U256, U256) = pair_abi.decode_output("getReserves", result.output)?;
    info!("Pair reserves: {:?}", reserves);

    let db = evm.db.as_mut().unwrap();

    let (balance_slot_0, balance_slot_1) = if token0 == input_token.into() {
        (input_balance_slot, output_balance_slot)
    } else {
        (output_balance_slot, input_balance_slot)
    };
    info!(
        "Balance slot 0: {:?} / slot 1: {:?}",
        balance_slot_0, balance_slot_1
    );

    let pair_token0_slot = keccak256(&abi::encode(&[
        abi::Token::Address(target_pair.into()),
        abi::Token::Uint(U256::from(balance_slot_0)),
    ]));
    db.insert_account_storage(token0, pair_token0_slot.into(), reserves.0.into())?;

    let pair_token1_slot = keccak256(&abi::encode(&[
        abi::Token::Address(target_pair.into()),
        abi::Token::Uint(U256::from(balance_slot_1)),
    ]));
    db.insert_account_storage(token1, pair_token1_slot.into(), reserves.1.into())?;

    // Check that balance is set correctly
    let token_abi = BaseContract::from(parse_abi(&[
        "function balanceOf(address) external view returns (uint256)",
    ])?);
    for token in vec![token0, token1] {
        let calldata = token_abi.encode("balanceOf", target_pair)?;
        evm.env.tx.caller = account.into();
        evm.env.tx.transact_to = TransactTo::Call(token);
        evm.env.tx.data = calldata.0;
        let result = match evm.transact_ref() {
            Ok(result) => result,
            Err(e) => return Err(anyhow!("EVM call failed: {:?}", e)),
        };
        let result = get_tx_result(result.result)?;
        let balance: U256 = token_abi.decode_output("balanceOf", result.output)?;
        info!("{:?}: {:?}", token, balance);
    }

    // feed simulator with input_token balance
    let db = evm.db.as_mut().unwrap();

    let slot_in = keccak256(&abi::encode(&[
        abi::Token::Address(simulator_address.into()),
        abi::Token::Uint(U256::from(input_balance_slot)),
    ]));
    db.insert_account_storage(input_token.into(), slot_in.into(), ten_eth)?;


    //run v2SimulateSwap
    let amount_in = U256::from(1)
    .checked_mul(U256::from(10).pow(U256::from(18)))
    .unwrap();

    let simulator_abi = BaseContract::from(
        parse_abi(&[
            "function v2SimulateSwap(uint256,address,address,address) external returns (uint256, uint256)",
        ])?
    );

    let calldata = simulator_abi.encode(
        "v2SimulateSwap",
        (amount_in, target_pair, input_token, output_token),
    )?;
    let v2_simulate_swap_tx = TxEnv {
        caller: account.into(),
        gas_limit: 5000000,
        gas_price: gas_price,
        gas_priority_fee: None,
        transact_to: TransactTo::Call(simulator_address.into()),
        value: rU256::ZERO,
        data: calldata.0,
        chain_id: None,
        nonce: None,
        access_list: Default::default(),
    };
    evm.env.tx = v2_simulate_swap_tx;

    let result = match evm.transact_commit() {
        Ok(result) => result,
        Err(e) => return Err(anyhow!("EVM call failed: {:?}", e)),
    };

    let result = get_tx_result(result)?;
    let out: (U256, U256) = simulator_abi.decode_output("v2SimulateSwap", result.output)?;
    info!("Amount out {:?}", out);

    Ok(out)
}

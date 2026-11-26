import { ApiPromise, WsProvider } from '@polkadot/api';
import { web3Enable, web3Accounts, web3FromAddress } from '@polkadot/extension-dapp';
window.IctWallet = { ApiPromise, WsProvider, web3Enable, web3Accounts, web3FromAddress };

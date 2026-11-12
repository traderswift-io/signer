import json
import re

import aiohttp

from coordinator.base_transaction_digest import expected_transaction, raw_signed_transaction, recover_transaction_sender, signing_hash
from coordinator.chain_reader import read_policy
from coordinator.coordinator_rounds import log_event, now_milliseconds, resolve_venue_address
from coordinator.hub_owner_check import hub_owner_refusal
from coordinator.signer_client import ask_signers

MILLISECONDS_PER_SECOND = 1000
RPC_TIMEOUT_SECONDS = 10
HIGHEST_SIGNED_CHAIN_NONCE_REFUSAL = re.compile(r'chain_nonce_not_above_highest_signed_([0-9]+)')


async def call_base_rpc(http_session: aiohttp.ClientSession, rpc_url: str, method: str, params: list) -> dict:
    body = {'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params}
    try:
        async with http_session.post(rpc_url, json=body, timeout=aiohttp.ClientTimeout(total=RPC_TIMEOUT_SECONDS)) as response:
            return await response.json()
    except (aiohttp.ClientError, TimeoutError, ValueError) as cut:
        return {'error': {'message': type(cut).__name__}}


async def read_chain_nonce(http_session: aiohttp.ClientSession, rpc_url: str, address: str) -> int | None:
    answer = await call_base_rpc(http_session, rpc_url, 'eth_getTransactionCount', [address, 'pending'])
    result = answer.get('result')
    return int(result, 16) if isinstance(result, str) else None


async def prepare_base_round(state: dict, delegate_key: str) -> dict:
    config = state['config']
    owner_refusal = hub_owner_refusal(state)
    if owner_refusal is not None:
        return {'prepared': False, 'reason': owner_refusal}
    if not config.base_rpc_url:
        return {'prepared': False, 'reason': 'base_not_configured'}
    policy = await read_policy(state['http_session'], config.chain_rest_urls, config.key_ref, delegate_key)
    if policy is None:
        return {'prepared': False, 'reason': 'policy_unknown'}
    venue_address = await resolve_venue_address(state)
    if venue_address is None:
        return {'prepared': False, 'reason': 'venue_address_unknown'}
    chain_nonce = await read_chain_nonce(state['http_session'], config.base_rpc_url, venue_address)
    if chain_nonce is None:
        return {'prepared': False, 'reason': 'chain_nonce_unavailable'}
    chain_nonce = max(chain_nonce, state.get('highest_issued_base_nonce', -1) + 1)
    state['highest_issued_base_nonce'] = chain_nonce
    return {
        'prepared': True,
        'key_ref': config.key_ref,
        'policy_epoch': policy['epoch'],
        'chain_nonce': chain_nonce,
        'expires_at': now_milliseconds() // MILLISECONDS_PER_SECOND + config.intent_lifetime_seconds,
    }


async def broadcast_raw_transaction(state: dict, raw_transaction: str) -> tuple[str, dict]:
    config = state['config']
    if config.allow_live_base_broadcast is not True or not config.base_rpc_url.startswith('https://'):
        return 'not_sent', {'reason': 'live_base_broadcast_off'}
    answer = await call_base_rpc(state['http_session'], config.base_rpc_url, 'eth_sendRawTransaction', [raw_transaction])
    return ('ok' if 'result' in answer else 'err'), answer


def raise_base_nonce_floor_from_refusal(state: dict, reason: str) -> None:
    refusal = HIGHEST_SIGNED_CHAIN_NONCE_REFUSAL.search(reason)
    if refusal:
        state['highest_issued_base_nonce'] = max(state.get('highest_issued_base_nonce', -1), int(refusal.group(1)))


def build_expected_or_none(intent: dict) -> dict | None:
    try:
        return expected_transaction(intent)
    except (KeyError, ValueError, TypeError):
        return None


async def run_base_signing_round(state: dict, intent: dict) -> dict:
    config = state['config']
    request_id = intent.get('request_id', 'unnamed')
    venue_address = await resolve_venue_address(state)
    if venue_address is None:
        return {'request_id': request_id, 'signed': False, 'reason': 'venue_address_unknown'}
    expected = build_expected_or_none(intent)
    reply = await ask_signers(state['http_session'], config.signer_urls, intent)
    if not reply.get('signed'):
        reason = reply.get('reason', 'no_reason')
        raise_base_nonce_floor_from_refusal(state, reason)
        log_event('base_round_refused', request_id, kind=intent.get('kind'), reason=reason)
        return {'request_id': request_id, 'signed': False, 'reason': reason}
    if expected is None or json.dumps(expected) != json.dumps(reply.get('transaction')):
        log_event('base_round_transaction_mismatch', request_id)
        return {'request_id': request_id, 'signed': False, 'reason': 'transaction_is_not_the_expected_one'}
    digest = signing_hash(expected)
    if digest.hex() != reply.get('digest'):
        log_event('base_round_digest_mismatch', request_id)
        return {'request_id': request_id, 'signed': False, 'reason': 'digest_is_not_the_expected_one'}
    recovered = recover_transaction_sender(expected, reply['r'], reply['s'], reply['v'])
    if recovered != venue_address:
        log_event('base_round_wrong_signer', request_id, recovered=recovered)
        return {'request_id': request_id, 'signed': False, 'reason': 'signature_does_not_recover_to_venue_address'}
    raw_transaction = '0x' + raw_signed_transaction(expected, reply['r'], reply['s'], reply['v']).hex()
    if raw_transaction != reply.get('raw_transaction'):
        log_event('base_round_raw_mismatch', request_id)
        return {'request_id': request_id, 'signed': False, 'reason': 'raw_transaction_is_not_the_expected_one'}
    venue_status, venue_answer = await broadcast_raw_transaction(state, raw_transaction)
    log_event('base_round_signed', request_id, kind=intent['kind'], venue_status=venue_status, transaction_hash=reply.get('transaction_hash'))
    return {'request_id': request_id, 'signed': True, 'digest': reply['digest'], 'transaction': expected, 'raw_transaction': raw_transaction,
            'transaction_hash': reply.get('transaction_hash'), 'recovered': recovered, 'venue_status': venue_status,
            'venue_answer': venue_answer, 'asked': reply.get('asked')}

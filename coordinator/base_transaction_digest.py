import re
from decimal import Decimal

from coordinator.venue_digest import keccak256, recover_address

BASE_CHAIN_ID = 8453
BASE_VENUE_PREFIX = 'base_'
EIP1559_TRANSACTION_TYPE = b'\x02'
WORD_BYTES = 32
PLAIN_UNSIGNED_FORMAT = re.compile(r'^(0|[1-9][0-9]{0,38})$')
MAXIMUM_U128 = (1 << 128) - 1

ERC20_APPROVE_SELECTOR = bytes.fromhex('095ea7b3')
EXAMPLE_VENUE_CONTRACT = bytes.fromhex('00000000000000000000000000000000000e0001')
EXAMPLE_COLLATERAL_TOKEN = bytes.fromhex('00000000000000000000000000000000000e0002')
EXAMPLE_PLACE_ORDER_SELECTOR = bytes.fromhex('cc88135d')
EXAMPLE_SIZE_SCALE = 6

AVANTIS_TRADING_ROUTER = bytes.fromhex('44914408af82bc9983bbb330e3578e1105e11d4e')
AVANTIS_DELEGATED_ACTION_SELECTOR = bytes.fromhex('9a10cc32')
AVANTIS_OPEN_TRADE_SELECTOR = bytes.fromhex('19cde9a1')
AVANTIS_CLOSE_TRADE_MARKET_SELECTOR = bytes.fromhex('e5a97c92')
AVANTIS_CANCEL_OPEN_LIMIT_ORDER_SELECTOR = bytes.fromhex('b9b6573a')
AVANTIS_OPEN_ORDER_TYPES = (0, 1, 2)
AVANTIS_OPEN_TRADE_ARGUMENTS = ('pair_index', 'trade_index', 'collateral_usdc', 'open_price', 'is_long', 'leverage', 'take_profit',
                                'stop_loss', 'order_type', 'slippage')
AVANTIS_CLOSE_TRADE_MARKET_ARGUMENTS = ('pair_index', 'trade_index', 'collateral_to_close_usdc', 'wanted_price')
AVANTIS_CANCEL_OPEN_LIMIT_ORDER_ARGUMENTS = ('pair_index', 'trade_index')
AVANTIS_USDC_SCALE = 6
AVANTIS_LEVERAGE_SCALE = 10


def is_base_venue(venue) -> bool:
    return isinstance(venue, str) and venue.startswith(BASE_VENUE_PREFIX)


def parse_address_text(text: str) -> bytes:
    if not isinstance(text, str) or not re.fullmatch(r'0x[0-9a-f]{40}', text):
        raise ValueError('bad_address')
    return bytes.fromhex(text[2:])


def refuse_unexpected_arguments(arguments: dict, expected_names: tuple[str, ...]) -> None:
    unexpected = [name for name in arguments if name not in expected_names]
    if unexpected:
        raise ValueError(f'unexpected_argument_{sorted(unexpected)[0]}')


def unsigned_argument(arguments: dict, field_name: str) -> int:
    text = arguments.get(field_name)
    if text is None:
        raise ValueError(f'missing_{field_name}')
    if not isinstance(text, str) or not PLAIN_UNSIGNED_FORMAT.match(text) or int(text) > MAXIMUM_U128:
        raise ValueError(f'bad_{field_name}')
    return int(text)


def bool_argument(arguments: dict, field_name: str) -> bool:
    text = arguments.get(field_name)
    if text is None:
        raise ValueError(f'missing_{field_name}')
    if text not in ('true', 'false'):
        raise ValueError(f'bad_{field_name}')
    return text == 'true'


def word_from_unsigned(value: int) -> bytes:
    return value.to_bytes(WORD_BYTES, 'big')


def word_from_address(address: bytes) -> bytes:
    return bytes(12) + address


def build_example_call(kind: str, trader: bytes, arguments: dict) -> tuple[bytes, bytes, int, tuple[str, Decimal] | None]:
    if kind == 'place_order':
        refuse_unexpected_arguments(arguments, ('market_id', 'is_buy', 'size', 'limit_price'))
        market_id = unsigned_argument(arguments, 'market_id')
        size = unsigned_argument(arguments, 'size')
        if size == 0:
            raise ValueError('bad_size')
        data = EXAMPLE_PLACE_ORDER_SELECTOR + word_from_unsigned(market_id) + word_from_unsigned(int(bool_argument(arguments, 'is_buy'))) \
            + word_from_unsigned(size) + word_from_unsigned(unsigned_argument(arguments, 'limit_price'))
        return EXAMPLE_VENUE_CONTRACT, data, 0, (f'example:{market_id}', Decimal(size).scaleb(-EXAMPLE_SIZE_SCALE))
    if kind == 'approve_collateral':
        refuse_unexpected_arguments(arguments, ('amount',))
        data = ERC20_APPROVE_SELECTOR + word_from_address(EXAMPLE_VENUE_CONTRACT) + word_from_unsigned(unsigned_argument(arguments, 'amount'))
        return EXAMPLE_COLLATERAL_TOKEN, data, 0, None
    raise ValueError('unknown_kind')


def avantis_open_trade_inner(trader: bytes, arguments: dict) -> tuple[bytes, tuple[str, Decimal]]:
    refuse_unexpected_arguments(arguments, AVANTIS_OPEN_TRADE_ARGUMENTS)
    pair_index = unsigned_argument(arguments, 'pair_index')
    collateral_usdc = unsigned_argument(arguments, 'collateral_usdc')
    leverage = unsigned_argument(arguments, 'leverage')
    order_type = unsigned_argument(arguments, 'order_type')
    if collateral_usdc == 0 or leverage == 0:
        raise ValueError('bad_collateral_or_leverage')
    if order_type not in AVANTIS_OPEN_ORDER_TYPES:
        raise ValueError('bad_order_type')
    words = [
        word_from_address(trader),
        word_from_unsigned(pair_index),
        word_from_unsigned(unsigned_argument(arguments, 'trade_index')),
        word_from_unsigned(0),
        word_from_unsigned(collateral_usdc),
        word_from_unsigned(unsigned_argument(arguments, 'open_price')),
        word_from_unsigned(int(bool_argument(arguments, 'is_long'))),
        word_from_unsigned(leverage),
        word_from_unsigned(unsigned_argument(arguments, 'take_profit')),
        word_from_unsigned(unsigned_argument(arguments, 'stop_loss')),
        word_from_unsigned(0),
        word_from_unsigned(order_type),
        word_from_unsigned(unsigned_argument(arguments, 'slippage')),
    ]
    notional = Decimal(collateral_usdc).scaleb(-AVANTIS_USDC_SCALE) * Decimal(leverage).scaleb(-AVANTIS_LEVERAGE_SCALE)
    return AVANTIS_OPEN_TRADE_SELECTOR + b''.join(words), (f'avantis:{pair_index}', notional.normalize())


def avantis_plain_inner(arguments: dict, selector: bytes, names: tuple[str, ...]) -> bytes:
    refuse_unexpected_arguments(arguments, names)
    return selector + b''.join(word_from_unsigned(unsigned_argument(arguments, name)) for name in names)


def wrap_in_delegated_action(trader: bytes, inner_calldata: bytes) -> bytes:
    padding = bytes(-len(inner_calldata) % WORD_BYTES)
    return AVANTIS_DELEGATED_ACTION_SELECTOR + word_from_address(trader) + word_from_unsigned(2 * WORD_BYTES) \
        + word_from_unsigned(len(inner_calldata)) + inner_calldata + padding


def build_avantis_call(kind: str, trader: bytes, arguments: dict) -> tuple[bytes, bytes, int, tuple[str, Decimal] | None]:
    new_risk = None
    if kind == 'open_trade':
        inner_calldata, new_risk = avantis_open_trade_inner(trader, arguments)
    elif kind == 'close_trade_market':
        inner_calldata = avantis_plain_inner(arguments, AVANTIS_CLOSE_TRADE_MARKET_SELECTOR, AVANTIS_CLOSE_TRADE_MARKET_ARGUMENTS)
    elif kind == 'cancel_open_limit_order':
        inner_calldata = avantis_plain_inner(arguments, AVANTIS_CANCEL_OPEN_LIMIT_ORDER_SELECTOR, AVANTIS_CANCEL_OPEN_LIMIT_ORDER_ARGUMENTS)
    else:
        raise ValueError('unknown_kind')
    return AVANTIS_TRADING_ROUTER, wrap_in_delegated_action(trader, inner_calldata), 0, new_risk


BASE_CALL_BUILDERS = {
    'base_example': build_example_call,
    'base_avantis': build_avantis_call,
}


def expected_transaction(intent: dict) -> dict:
    build_call = BASE_CALL_BUILDERS.get(intent['venue'])
    if build_call is None:
        raise ValueError('venue_not_allowed')
    to, data, value_wei, _ = build_call(intent['kind'], parse_address_text(intent['vault_address']), intent['arguments'])
    return {
        'chain_id': BASE_CHAIN_ID,
        'nonce': intent['chain_nonce'],
        'max_priority_fee_per_gas': intent['max_priority_fee_per_gas'],
        'max_fee_per_gas': intent['max_fee_per_gas'],
        'gas_limit': intent['gas_limit'],
        'to': '0x' + to.hex(),
        'value_wei': str(value_wei),
        'data': '0x' + data.hex(),
    }


def rlp_length_prefix(payload_length: int, short_offset: int, long_offset: int) -> bytes:
    if payload_length <= 55:
        return bytes([short_offset + payload_length])
    length_bytes = payload_length.to_bytes((payload_length.bit_length() + 7) // 8, 'big')
    return bytes([long_offset + len(length_bytes)]) + length_bytes


def rlp_encode_bytes(data: bytes) -> bytes:
    if len(data) == 1 and data[0] < 0x80:
        return data
    return rlp_length_prefix(len(data), 0x80, 0xb7) + data


def rlp_encode_unsigned(value: int) -> bytes:
    return rlp_encode_bytes(value.to_bytes((value.bit_length() + 7) // 8, 'big'))


def rlp_wrap_list(payload: bytes) -> bytes:
    return rlp_length_prefix(len(payload), 0xc0, 0xf7) + payload


def unsigned_fields_payload(transaction: dict) -> bytes:
    return b''.join([
        rlp_encode_unsigned(transaction['chain_id']),
        rlp_encode_unsigned(transaction['nonce']),
        rlp_encode_unsigned(transaction['max_priority_fee_per_gas']),
        rlp_encode_unsigned(transaction['max_fee_per_gas']),
        rlp_encode_unsigned(transaction['gas_limit']),
        rlp_encode_bytes(bytes.fromhex(transaction['to'][2:])),
        rlp_encode_unsigned(int(transaction['value_wei'])),
        rlp_encode_bytes(bytes.fromhex(transaction['data'][2:])),
        rlp_wrap_list(b''),
    ])


def signing_hash(transaction: dict) -> bytes:
    return keccak256(EIP1559_TRANSACTION_TYPE + rlp_wrap_list(unsigned_fields_payload(transaction)))


def raw_signed_transaction(transaction: dict, r_hex: str, s_hex: str, v: int) -> bytes:
    y_parity = v - 27 if v in (27, 28) else v
    if y_parity not in (0, 1):
        raise ValueError(f'bad_recovery_value_{v}')
    signature_fields = rlp_encode_unsigned(y_parity) + rlp_encode_unsigned(int(r_hex, 16)) + rlp_encode_unsigned(int(s_hex, 16))
    return EIP1559_TRANSACTION_TYPE + rlp_wrap_list(unsigned_fields_payload(transaction) + signature_fields)


def recover_transaction_sender(transaction: dict, r_hex: str, s_hex: str, v: int) -> str:
    return recover_address(signing_hash(transaction), r_hex, s_hex, v if v in (27, 28) else v + 27)

package validatorset

import (
	"context"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"math/big"
	"net"
	"regexp"
	"strconv"

	"cosmossdk.io/collections"
	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"
	upgradeplan "github.com/cosmos/cosmos-sdk/x/upgrade/plan"
)

const (
	RequestJoinProofDomain          = "signer-request-join-v1"
	defaultOutsiderCountCap         = uint32(100)
	defaultJoinerPower              = int64(100_000)
	defaultMissedBlocksBeforeRemove = int64(300)
	defaultRejoinCooldownSeconds    = int64(86_400)
	maximumContactLength            = 200
)

var scheduledOpenJoinActivationHeights = map[string]int64{}

var SupportedUpgradePlatforms = []string{"linux/amd64"}

var nodeIdPattern = regexp.MustCompile(`^[0-9a-f]{40}$`)

func defaultOpenJoinParams(chainId string) OpenJoinParams {
	return OpenJoinParams{
		ActivationHeight:          scheduledOpenJoinActivationHeights[chainId],
		OutsiderCountCap:          defaultOutsiderCountCap,
		JoinerPower:               defaultJoinerPower,
		MissedBlocksBeforeRemoval: defaultMissedBlocksBeforeRemove,
		RejoinCooldownSeconds:     defaultRejoinCooldownSeconds,
	}
}

func validateOpenJoinParams(params OpenJoinParams) error {
	if params.ActivationHeight < 0 || params.OutsiderCountCap == 0 || params.MissedBlocksBeforeRemoval < 1 || params.RejoinCooldownSeconds < 0 {
		return errorsmod.Wrap(ErrInvalidField, "open_join_params: activation_height >= 0, outsider_count_cap >= 1, missed_blocks_before_removal >= 1, rejoin_cooldown_seconds >= 0")
	}
	return validatePower(params.JoinerPower)
}

func (keeper *Keeper) openJoinParams(ctx context.Context) (OpenJoinParams, error) {
	params, err := keeper.StoredOpenJoinParams.Get(ctx)
	if errors.Is(err, collections.ErrNotFound) {
		return defaultOpenJoinParams(sdk.UnwrapSDKContext(ctx).ChainID()), nil
	}
	return params, err
}

func (keeper *Keeper) openJoinIsActive(ctx context.Context) (bool, OpenJoinParams, error) {
	params, err := keeper.openJoinParams(ctx)
	if err != nil {
		return false, params, err
	}
	height := sdk.UnwrapSDKContext(ctx).BlockHeight()
	return params.ActivationHeight > 0 && height >= params.ActivationHeight, params, nil
}

func appendLengthPrefixed(buffer []byte, field []byte) []byte {
	buffer = binary.AppendUvarint(buffer, uint64(len(field)))
	return append(buffer, field...)
}

func RequestJoinProofBytes(chainId string, requester string, nodeId string, consensusPublicKey []byte) []byte {
	buffer := []byte(RequestJoinProofDomain)
	buffer = appendLengthPrefixed(buffer, []byte(chainId))
	buffer = appendLengthPrefixed(buffer, []byte(requester))
	buffer = appendLengthPrefixed(buffer, []byte(nodeId))
	return appendLengthPrefixed(buffer, consensusPublicKey)
}

func consensusAddressOf(consensusPublicKey []byte) string {
	digest := sha256.Sum256(consensusPublicKey)
	return hex.EncodeToString(digest[:20])
}

func validateEndpointIp(endpointIp string) error {
	address := net.ParseIP(endpointIp)
	if address == nil || address.String() != endpointIp || !address.IsGlobalUnicast() || address.IsPrivate() {
		return errorsmod.Wrap(ErrInvalidField, "endpoint_ip must be a public IP address in canonical form")
	}
	return nil
}

func validateRequestJoinFields(message *MsgRequestJoin) ([]byte, error) {
	if _, err := sdk.AccAddressFromBech32(message.Requester); err != nil {
		return nil, errorsmod.Wrapf(ErrInvalidField, "requester %q: %v", message.Requester, err)
	}
	publicKeyBytes, err := decodeConsensusPublicKey(message.ConsensusPublicKey)
	if err != nil {
		return nil, err
	}
	if !nodeIdPattern.MatchString(message.NodeId) {
		return nil, errorsmod.Wrap(ErrInvalidField, "node_id must be 40 lowercase hex characters")
	}
	if err := validateMoniker(message.Moniker); err != nil {
		return nil, err
	}
	if err := validateEndpointIp(message.EndpointIp); err != nil {
		return nil, err
	}
	if len(message.Contact) > maximumContactLength {
		return nil, errorsmod.Wrapf(ErrInvalidField, "contact must be at most %d bytes", maximumContactLength)
	}
	return publicKeyBytes, nil
}

func VerifyRequestJoinProof(chainId string, message *MsgRequestJoin) error {
	publicKeyBytes, err := validateRequestJoinFields(message)
	if err != nil {
		return err
	}
	signature, err := base64.StdEncoding.DecodeString(message.ConsensusKeyProof)
	if err != nil || len(signature) != ed25519.SignatureSize {
		return errorsmod.Wrap(ErrJoinProofInvalid, "consensus_key_proof must be a 64-byte ed25519 signature in standard base64")
	}
	if !ed25519.Verify(ed25519.PublicKey(publicKeyBytes), RequestJoinProofBytes(chainId, message.Requester, message.NodeId, publicKeyBytes), signature) {
		return ErrJoinProofInvalid
	}
	return nil
}

type outsiderTotals struct {
	count          uint32
	outsiderPower  *big.Int
	totalPower     *big.Int
	founderPower   *big.Int
	largestFounder *big.Int
}

func totalsOf(validators []Validator) outsiderTotals {
	totals := outsiderTotals{outsiderPower: new(big.Int), totalPower: new(big.Int), founderPower: new(big.Int), largestFounder: new(big.Int)}
	for _, validator := range validators {
		power := big.NewInt(validator.Power)
		totals.totalPower.Add(totals.totalPower, power)
		if !validator.Founding {
			totals.count++
			totals.outsiderPower.Add(totals.outsiderPower, power)
			continue
		}
		totals.founderPower.Add(totals.founderPower, power)
		if power.Cmp(totals.largestFounder) > 0 {
			totals.largestFounder = power
		}
	}
	return totals
}

// The chain must keep committing with its largest founder down and every outsider down:
// Fsum - fmax > 2/3 (Fsum + O), which is 2·O < Fsum - 3·fmax.
func (totals outsiderTotals) livenessLimitTimesTwo() *big.Int {
	return new(big.Int).Sub(totals.founderPower, new(big.Int).Mul(totals.largestFounder, big.NewInt(3)))
}

func (totals outsiderTotals) outsiderPowerLimit() int64 {
	limit := new(big.Int).Quo(totals.livenessLimitTimesTwo(), big.NewInt(2))
	if limit.Sign() < 0 {
		return 0
	}
	return limit.Int64()
}

func (keeper *Keeper) refusalForRequest(ctx context.Context, message *MsgRequestJoin, params OpenJoinParams) (string, error) {
	present, err := keeper.Validators.Has(ctx, message.ConsensusPublicKey)
	if err != nil {
		return "", err
	}
	if present {
		return "consensus_key_already_a_validator", nil
	}
	if taken, err := keeper.JoinerKeyByOwner.Has(ctx, message.Requester); err != nil || taken {
		return "account_already_has_a_validator", err
	}
	if taken, err := keeper.JoinerKeyByEndpointIp.Has(ctx, message.EndpointIp); err != nil || taken {
		return "endpoint_ip_already_has_a_validator", err
	}
	blockTime := sdk.UnwrapSDKContext(ctx).BlockTime().Unix()
	for _, cooldownKey := range []string{message.Requester, message.ConsensusPublicKey} {
		until, err := keeper.RejoinCooldownUntil.Get(ctx, cooldownKey)
		if err == nil && blockTime < until {
			return "rejoin_cooldown_until_" + strconv.FormatInt(until, 10), nil
		}
		if err != nil && !errors.Is(err, collections.ErrNotFound) {
			return "", err
		}
	}
	validators, err := keeper.allValidators(ctx)
	if err != nil {
		return "", err
	}
	totals := totalsOf(validators)
	if totals.count+1 > params.OutsiderCountCap {
		return "outsider_count_cap_" + strconv.FormatUint(uint64(params.OutsiderCountCap), 10) + "_reached", nil
	}
	outsiderPowerAfter := new(big.Int).Add(totals.outsiderPower, big.NewInt(params.JoinerPower))
	if new(big.Int).Mul(outsiderPowerAfter, big.NewInt(2)).Cmp(totals.livenessLimitTimesTwo()) >= 0 {
		return "outsider_power_" + outsiderPowerAfter.String() + "_would_reach_liveness_limit_" + strconv.FormatInt(totals.outsiderPowerLimit(), 10), nil
	}
	return "", nil
}

func (keeper *Keeper) acceptRequest(ctx context.Context, message *MsgRequestJoin, publicKeyBytes []byte, params OpenJoinParams) (string, error) {
	validator := Validator{ConsensusPublicKey: message.ConsensusPublicKey, Power: params.JoinerPower, Moniker: message.Moniker, Founding: false}
	if err := keeper.applyChange(ctx, message.ConsensusPublicKey, &validator); err != nil {
		return err.Error(), nil
	}
	return "", keeper.recordJoiner(ctx, message, publicKeyBytes)
}

func (keeper *Keeper) recordJoiner(ctx context.Context, message *MsgRequestJoin, publicKeyBytes []byte) error {
	joiner := Joiner{
		ConsensusPublicKey: message.ConsensusPublicKey,
		OwnerAddress:       message.Requester,
		NodeId:             message.NodeId,
		EndpointIp:         message.EndpointIp,
		Contact:            message.Contact,
		JoinedAtHeight:     sdk.UnwrapSDKContext(ctx).BlockHeight(),
	}
	if err := keeper.Joiners.Set(ctx, joiner.ConsensusPublicKey, joiner); err != nil {
		return err
	}
	if err := keeper.JoinerKeyByOwner.Set(ctx, joiner.OwnerAddress, joiner.ConsensusPublicKey); err != nil {
		return err
	}
	if err := keeper.JoinerKeyByEndpointIp.Set(ctx, joiner.EndpointIp, joiner.ConsensusPublicKey); err != nil {
		return err
	}
	return keeper.JoinerKeyByConsensusAddress.Set(ctx, consensusAddressOf(publicKeyBytes), joiner.ConsensusPublicKey)
}

func (keeper *Keeper) forgetJoiner(ctx context.Context, joiner Joiner) error {
	publicKeyBytes, err := decodeConsensusPublicKey(joiner.ConsensusPublicKey)
	if err != nil {
		return err
	}
	if err := keeper.Joiners.Remove(ctx, joiner.ConsensusPublicKey); err != nil {
		return err
	}
	if err := keeper.JoinerKeyByOwner.Remove(ctx, joiner.OwnerAddress); err != nil {
		return err
	}
	if err := keeper.JoinerKeyByEndpointIp.Remove(ctx, joiner.EndpointIp); err != nil {
		return err
	}
	return keeper.JoinerKeyByConsensusAddress.Remove(ctx, consensusAddressOf(publicKeyBytes))
}

func (keeper *Keeper) forgetJoinerIfPresent(ctx context.Context, consensusPublicKey string) error {
	joiner, err := keeper.Joiners.Get(ctx, consensusPublicKey)
	if errors.Is(err, collections.ErrNotFound) {
		return nil
	}
	if err != nil {
		return err
	}
	return keeper.forgetJoiner(ctx, joiner)
}

func (keeper *Keeper) removeJoinerForMissedBlocks(ctx context.Context, joiner Joiner, params OpenJoinParams) error {
	if err := keeper.applyChange(ctx, joiner.ConsensusPublicKey, nil); err != nil {
		return err
	}
	if err := keeper.forgetJoiner(ctx, joiner); err != nil {
		return err
	}
	until := sdk.UnwrapSDKContext(ctx).BlockTime().Unix() + params.RejoinCooldownSeconds
	if err := keeper.RejoinCooldownUntil.Set(ctx, joiner.OwnerAddress, until); err != nil {
		return err
	}
	if err := keeper.RejoinCooldownUntil.Set(ctx, joiner.ConsensusPublicKey, until); err != nil {
		return err
	}
	emitValidatorSetEvent(ctx, "validator_removed_for_missed_blocks",
		sdk.NewAttribute("consensus_public_key", joiner.ConsensusPublicKey),
		sdk.NewAttribute("owner_address", joiner.OwnerAddress),
		sdk.NewAttribute("consecutive_missed_blocks", strconv.FormatInt(joiner.ConsecutiveMissedBlocks, 10)),
		sdk.NewAttribute("rejoin_allowed_from_unix_seconds", strconv.FormatInt(until, 10)))
	return nil
}

func (keeper *Keeper) TrackMissedBlocks(ctx context.Context) error {
	active, params, err := keeper.openJoinIsActive(ctx)
	if err != nil || !active {
		return err
	}
	votes := sdk.UnwrapSDKContext(ctx).CometInfo().GetLastCommit().Votes()
	for index := 0; index < votes.Len(); index++ {
		vote := votes.Get(index)
		consensusPublicKey, err := keeper.JoinerKeyByConsensusAddress.Get(ctx, hex.EncodeToString(vote.Validator().Address()))
		if errors.Is(err, collections.ErrNotFound) {
			continue
		}
		if err != nil {
			return err
		}
		joiner, err := keeper.Joiners.Get(ctx, consensusPublicKey)
		if err != nil {
			return err
		}
		signed := vote.GetBlockIDFlag() == 2
		if signed && joiner.ConsecutiveMissedBlocks == 0 {
			continue
		}
		if signed {
			joiner.ConsecutiveMissedBlocks = 0
		} else {
			joiner.ConsecutiveMissedBlocks++
		}
		if joiner.ConsecutiveMissedBlocks >= params.MissedBlocksBeforeRemoval {
			if err := keeper.removeJoinerForMissedBlocks(ctx, joiner, params); err != nil {
				return err
			}
			continue
		}
		if err := keeper.Joiners.Set(ctx, joiner.ConsensusPublicKey, joiner); err != nil {
			return err
		}
	}
	return nil
}

func validateUpgradeInfo(info string) error {
	parsed, err := upgradeplan.ParseInfo(info, upgradeplan.ParseOptionEnforceChecksum(true))
	if err != nil {
		return errorsmod.Wrapf(ErrInvalidField, "upgrade info: %v", err)
	}
	if err := parsed.Binaries.ValidateBasic(true); err != nil {
		return errorsmod.Wrapf(ErrInvalidField, "upgrade info: %v", err)
	}
	for _, platform := range SupportedUpgradePlatforms {
		if _, present := parsed.Binaries[platform]; !present {
			return errorsmod.Wrapf(ErrInvalidField, "upgrade info must carry a binary for %s", platform)
		}
	}
	return nil
}

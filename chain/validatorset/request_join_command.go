package validatorset

import (
	"encoding/base64"
	"path/filepath"

	"github.com/cometbft/cometbft/p2p"
	"github.com/cometbft/cometbft/privval"
	"github.com/spf13/cobra"

	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/client/tx"
)

const (
	flagMoniker           = "moniker"
	flagEndpointIp        = "endpoint-ip"
	flagContact           = "contact"
	defaultRequestJoinGas = uint64(400_000)
)

func newTransactionCommand() *cobra.Command {
	command := &cobra.Command{Use: ModuleName, Short: "Validator set transactions", RunE: client.ValidateCmd}
	command.AddCommand(newRequestJoinCommand())
	return command
}

func buildRequestJoinMessage(chainId string, requester string, homeDirectory string, moniker string, endpointIp string, contact string) (*MsgRequestJoin, error) {
	privateValidator := privval.LoadFilePVEmptyState(filepath.Join(homeDirectory, "config", "priv_validator_key.json"), "")
	nodeKey, err := p2p.LoadNodeKey(filepath.Join(homeDirectory, "config", "node_key.json"))
	if err != nil {
		return nil, err
	}
	publicKeyBytes := privateValidator.Key.PubKey.Bytes()
	nodeId := string(nodeKey.ID())
	signature, err := privateValidator.Key.PrivKey.Sign(RequestJoinProofBytes(chainId, requester, nodeId, publicKeyBytes))
	if err != nil {
		return nil, err
	}
	return &MsgRequestJoin{
		Requester:          requester,
		ConsensusPublicKey: base64.StdEncoding.EncodeToString(publicKeyBytes),
		NodeId:             nodeId,
		Moniker:            moniker,
		EndpointIp:         endpointIp,
		Contact:            contact,
		ConsensusKeyProof:  base64.StdEncoding.EncodeToString(signature),
	}, nil
}

func newRequestJoinCommand() *cobra.Command {
	command := &cobra.Command{
		Use:   "request-join",
		Short: "Ask to join the validator set; signs the request with this node's consensus key, which never leaves the machine",
		Args:  cobra.NoArgs,
		RunE: func(command *cobra.Command, _ []string) error {
			clientContext, err := client.GetClientTxContext(command)
			if err != nil {
				return err
			}
			moniker, _ := command.Flags().GetString(flagMoniker)
			endpointIp, _ := command.Flags().GetString(flagEndpointIp)
			contact, _ := command.Flags().GetString(flagContact)
			message, err := buildRequestJoinMessage(clientContext.ChainID, clientContext.GetFromAddress().String(), clientContext.HomeDir, moniker, endpointIp, contact)
			if err != nil {
				return err
			}
			if err := VerifyRequestJoinProof(clientContext.ChainID, message); err != nil {
				return err
			}
			factory, err := tx.NewFactoryCLI(clientContext, command.Flags())
			if err != nil {
				return err
			}
			if !command.Flags().Changed(flags.FlagGas) {
				factory = factory.WithGas(defaultRequestJoinGas)
			}
			if clientContext.AccountRetriever.EnsureExists(clientContext, clientContext.GetFromAddress()) == nil {
				return tx.GenerateOrBroadcastTxWithFactory(clientContext, factory, message)
			}
			factory = factory.WithAccountNumber(FirstContactAccountNumber(clientContext.GetFromAddress())).WithSequence(0).WithSimulateAndExecute(false)
			unsignedTransaction, err := factory.BuildUnsignedTx(message)
			if err != nil {
				return err
			}
			if err := tx.Sign(command.Context(), factory, clientContext.FromName, unsignedTransaction, true); err != nil {
				return err
			}
			transactionBytes, err := clientContext.TxConfig.TxEncoder()(unsignedTransaction.GetTx())
			if err != nil {
				return err
			}
			response, err := clientContext.BroadcastTx(transactionBytes)
			if err != nil {
				return err
			}
			return clientContext.PrintProto(response)
		},
	}
	command.Flags().String(flagMoniker, "", "public name for this validator")
	command.Flags().String(flagEndpointIp, "", "this node's public IP address; one validator per IP")
	command.Flags().String(flagContact, "", "optional contact, shown publicly")
	_ = command.MarkFlagRequired(flagMoniker)
	_ = command.MarkFlagRequired(flagEndpointIp)
	flags.AddTxFlagsToCmd(command)
	return command
}

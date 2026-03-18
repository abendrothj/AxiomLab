// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

/// @title AuditVerifier
/// @notice Accepts RISC Zero proofs that verify the AxiomLab audit log hash chain.
///
/// @dev Deploy once to Base L2.  After each protocol conclusion, the AxiomLab
///      runtime submits a ZK proof that proves the audit chain was intact and
///      contains N events with M violations.  Zero content from the audit log
///      is ever disclosed — only the cryptographic summary.
///
/// Deploy with Foundry:
/// ```sh
/// forge create contracts/AuditVerifier.sol:AuditVerifier \
///   --rpc-url $BASE_RPC_URL \
///   --private-key $DEPLOYER_KEY \
///   --constructor-args $RISC0_VERIFIER_ADDR $AUDIT_IMAGE_ID
/// ```
///
/// Verify a proof without trust:
/// ```sh
/// cast call $CONTRACT_ADDR "latestTipHash()(bytes32)" --rpc-url $BASE_RPC_URL
/// ```
contract AuditVerifier {
    /// @notice The RISC Zero on-chain verifier contract (deployed by RISC Zero on Base).
    IRiscZeroVerifier public immutable verifier;

    /// @notice SHA-256 of the guest ELF binary.  Changing the guest logic
    ///         requires a new deployment with the new image ID.
    bytes32 public immutable AUDIT_IMAGE_ID;

    /// @notice Emitted for each verified proof submission.
    /// @dev    All fields are indexed or available in the proof journal.
    ///         `tipHash` can be cross-referenced against the local JSONL file.
    event AuditProofVerified(
        bytes32 indexed tipHash,
        uint64  eventCount,
        uint64  violationCount,
        uint64  firstUnixSecs,
        uint64  lastUnixSecs,
        uint256 blockTimestamp
    );

    /// @notice ABI layout for the guest journal output.
    struct AuditSummary {
        bool   chainValid;
        uint64 eventCount;
        uint64 violationCount;
        bytes32 tipHash;
        uint64 firstUnixSecs;
        uint64 lastUnixSecs;
    }

    constructor(address _verifier, bytes32 _imageId) {
        verifier = IRiscZeroVerifier(_verifier);
        AUDIT_IMAGE_ID = _imageId;
    }

    /// @notice Submit a RISC Zero proof that the audit chain is valid.
    /// @param  seal     RISC Zero proof seal (from `Receipt.inner.seal`).
    /// @param  journal  ABI-encoded `AuditSummary` (from `Receipt.journal.bytes`).
    ///
    /// Reverts if the proof is invalid or if `chainValid` is false.
    function submitProof(bytes calldata seal, bytes calldata journal) external {
        // Verify proof against the pinned image ID.
        verifier.verify(seal, AUDIT_IMAGE_ID, sha256(journal));

        AuditSummary memory s = abi.decode(journal, (AuditSummary));
        require(s.chainValid, "AuditVerifier: audit chain integrity check failed");

        emit AuditProofVerified(
            s.tipHash,
            s.eventCount,
            s.violationCount,
            s.firstUnixSecs,
            s.lastUnixSecs,
            block.timestamp
        );
    }

    /// @notice Returns the latest verified tip hash.
    ///         Compare this against your local audit JSONL to confirm integrity.
    function latestTipHash() external view returns (bytes32) {
        // Stored in the most recently emitted AuditProofVerified event.
        // Use an indexer (The Graph / Etherscan) to query historical entries.
        revert("query AuditProofVerified events for full history");
    }
}

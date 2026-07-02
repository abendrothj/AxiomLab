import React from 'react';

export default function ApprovalToast({ approvals, onResolve }) {
  if (!approvals || approvals.length === 0) return null;

  return (
    <div className="approval-overlay">
      {approvals.map((ap) => (
        <div key={ap.id} className="approval-toast">
          <div className="approval-header">
            <span className="approval-icon">!</span>
            <span className="approval-title">Operator Approval Required</span>
          </div>
          <div className="approval-body">
            <div className="approval-detail">
              <span className="ap-label">Action</span>
              <span className="ap-value">{ap.tool}</span>
            </div>
            <div className="approval-detail">
              <span className="ap-label">Risk Class</span>
              <span className={`ap-value risk-${(ap.riskClass || '').toLowerCase()}`}>{ap.riskClass}</span>
            </div>
            <div className="approval-detail">
              <span className="ap-label">Reason</span>
              <span className="ap-value">{ap.reason}</span>
            </div>
          </div>
          <div className="approval-actions">
            <button className="ap-deny" onClick={() => onResolve(ap.id, false)}>Deny</button>
            <button className="ap-approve" onClick={() => onResolve(ap.id, true)}>Approve</button>
          </div>
        </div>
      ))}
    </div>
  );
}

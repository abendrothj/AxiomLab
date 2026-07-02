import React, { useState } from 'react';
import { api } from '../api';

export default function LoginOverlay({ onLogin }) {
  const [subject, setSubject] = useState('operator');
  const [role, setRole] = useState('operator');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const handleDevLogin = async (e) => {
    e.preventDefault();
    setLoading(true);
    setError('');
    try {
      const principal = await api.devLogin(subject, role);
      onLogin(principal);
    } catch (err) {
      setError('Login failed — is AXIOMLAB_DEV_AUTH=1 set on the server?');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="login-overlay">
      <div className="login-card">
        <div className="login-header">
          <span className="logo-text">Axiom<span className="logo-accent">Lab</span></span>
          <span className="login-subtitle">Operator Console</span>
        </div>
        <form onSubmit={handleDevLogin} className="login-form">
          <label className="login-field">
            <span>Subject</span>
            <input value={subject} onChange={e => setSubject(e.target.value)} placeholder="operator" />
          </label>
          <label className="login-field">
            <span>Role</span>
            <select value={role} onChange={e => setRole(e.target.value)}>
              <option value="viewer">Viewer</option>
              <option value="operator">Operator</option>
              <option value="approver">Approver</option>
              <option value="admin">Admin</option>
            </select>
          </label>
          {error && <div className="login-error">{error}</div>}
          <button type="submit" disabled={loading} className="login-btn">
            {loading ? 'Signing in…' : 'Sign in (Development)'}
          </button>
        </form>
        <a href="/api/auth/login" className="login-oidc">Sign in with OIDC</a>
      </div>
    </div>
  );
}

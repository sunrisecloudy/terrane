export function Header() {
  return (
    <header>
      <div className="brand">
        <div className="brand-mark" aria-hidden="true">
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M12 20.5C7.5 17.7 4 14.7 4 10.4 4 7.8 5.8 6 8.2 6c1.6 0 3 1 3.8 2.2C12.8 7 14.2 6 15.8 6 18.2 6 20 7.8 20 10.4c0 4.3-3.5 7.3-8 10.1Z" />
            <path d="M12 6c-.1-2 1.1-3.4 3.3-3.8" />
          </svg>
        </div>
        <div>
          <h1>Health</h1>
          <p className="subtitle">Understand what’s on your plate.</p>
        </div>
      </div>
      <div className="privacy">Photos stay in your Terrane home</div>
    </header>
  );
}

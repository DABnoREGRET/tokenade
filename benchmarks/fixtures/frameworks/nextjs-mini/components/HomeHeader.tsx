"use client";

import { useState } from "react";

export function HomeHeader() {
  const [open, setOpen] = useState(false);
  return (
    <header onClick={() => setOpen(!open)}>
      <h1>Welcome</h1>
      {open && <nav>Menu</nav>}
    </header>
  );
}

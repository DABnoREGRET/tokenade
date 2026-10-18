import { Suspense } from "react";
import { HomeHeader } from "@/components/HomeHeader";

export const metadata = {
  title: "SuperTokenSaver — Home",
};

export default function Home() {
  return (
    <main>
      <Suspense fallback={null}>
        <HomeHeader />
      </Suspense>
    </main>
  );
}

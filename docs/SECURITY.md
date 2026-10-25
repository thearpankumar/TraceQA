# Security policy

## Supported versions

TraceQA is below 1.0. Only the latest 0.x release gets security fixes.

## Reporting a vulnerability

Please do not open a public issue for a vulnerability.

Report it privately through GitHub:
<https://github.com/thearpankumar/TraceQA/security/advisories/new>

Include, as far as you can:

- what is affected (a command, the MCP server, the installer, a driver, a workflow) and the version;
- the steps to reproduce it, or a small proof of concept;
- what an attacker gains, and what they need first;
- any fix you have in mind.

You can expect an acknowledgement within 5 working days, an assessment within
14 days, and a fix or a plan for one after that. You will be told when it is
released, and credited in the advisory if you want to be.

## In scope

- the MCP server and what its tools return to the agent;
- the installer (`traceqa install`) and the drivers it fetches;
- run folders and what they store, including storage state (cookies and sessions);
- the browser drivers and the collector script that runs inside pages;
- the CI and release workflows, and the released binaries.

## Out of scope

- a site you test doing something to its own page while TraceQA runs on it;
- problems that need access to your machine or your user account already;
- vulnerabilities in a dependency with no way to reach them through TraceQA
  (report those upstream).

## Safe harbour

If you act in good faith, keep to your own accounts and data, and do not harm
others or disclose the issue before it is fixed, we will not take action
against you for the research.

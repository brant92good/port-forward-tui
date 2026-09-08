"""Explain common SSH failures while retaining the original diagnostic text."""


def connection_help(details):
    lower = details.lower()
    if 'already in use' in lower or 'address already' in lower:
        return 'Another app uses that port on this computer. Press E and change only the THIS computer port.'
    if 'permission denied' in lower:
        return 'SSH login was rejected. Try ssh YOUR_SSH_NAME in PowerShell; background connections need a loaded SSH key.'
    if 'host key verification failed' in lower or 'identification has changed' in lower:
        return 'SSH could not verify this computer. Run ssh YOUR_SSH_NAME and verify its identity with the server owner.'
    if 'could not resolve hostname' in lower:
        return 'The remote computer name was not found. Check the SSH name and whether your VPN is connected.'
    if 'timed out' in lower or 'no route' in lower or 'network is unreachable' in lower:
        return 'The remote computer could not be reached. Check its power, network and any required VPN connection.'
    if 'connection refused' in lower:
        return 'The destination refused the connection. Check the address, port, and whether the service is running.'
    return 'The connection did not open. Check the SSH details below; Enter retries after you fix the cause.'

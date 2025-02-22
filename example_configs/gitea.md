# Configuration for Gitea
In Gitea, go to `Site Administration > Authentication Sources` and click `Add Authentication Source`
Select `LDAP (via BindDN)`

* Host: Your lldap server's ip/hostname
* Port: Your lldap server's port (3890 by default)
* Bind DN: `uid=admin,ou=people,dc=example,dc=com`
* Bind Password: Your bind user's password
* User Search Base: `ou=people,dc=example,dc=com`
* User Filter:  If you want all users to be able to log in, use<br>
`(&(objectClass=person)(|(uid=%[1]s)(mail=%[1]s)))`.<br>
To log in they can either use their email address or user name. If you only want members a specific group to be able to log in, in this case the group `git_user`, use<br>
`(&(memberof=cn=git_user,ou=groups,dc=example,dc=com)(|(uid=%[1]s)(mail=%[1]s)))`<br>
For more info on the user filter, see: https://docs.gitea.io/en-us/authentication/#ldap-via-binddn
* Admin Filter: Use `(memberof=cn=lldap_admin,ou=groups,dc=example,dc=com)` if you want lldap admins to become Gitea admins. Leave empty otherwise.
* Username Attribute: `uid`
* Email Attribute: `mail`
* Check `Enable User Synchronization`

Replace every instance of `dc=example,dc=com` with your configured domain.

After applying the above settings, users should be able to log in with either their user name or email address.
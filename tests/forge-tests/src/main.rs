#![feature(register_tool)]
#![register_tool(dfpp)]

#[dfpp::label(sensitive)]
struct UserData {
    pub data: Vec<i64>,
}

#[dfpp::analyze]
fn process_if() {
    let user_data = get_user_data();
    if check_user_data(&user_data) {
        send_user_data(&user_data);
    }
}

#[dfpp::analyze]
fn process_invalid_check() {
    let user_data = get_user_data();
    check_user_data(&user_data);
    send_user_data(&user_data);
}

#[dfpp::label{source, return}]
fn get_user_data() -> UserData {
    return UserData{data: vec![1, 2, 3]}
}

#[dfpp::label{checks, return}]
fn check_user_data(user_data: &UserData) -> bool {
    for i in &user_data.data {
        if i < &0 {
            return false
        }
    }
    return true
}

#[dfpp::label{ sink, arguments = [0] }]
fn send_user_data(_user_data: &UserData) {
}

#[dfpp::analyze]
fn blessed_safe_sources(config: u8) {
	let mut recipients = if get_num() < 90 {
		get_staff(config)
	} else {
		get_admins(config)
	};
	send(recipients)
}

#[dfpp::analyze]
fn only_safe_sources(config: u8) {
	let mut recipients = get_admins(config);
	send(recipients)
}

#[dfpp::analyze]
fn unblessed_safe_sources_with_bless(config: u8) {
	let mut recipients = get_staff(config);
	send(recipients)
}

#[dfpp::analyze]
fn unsafe_sources(config: u8) {
	let mut recipients = if get_num() < 90 {
		get_staff(config)
	} else {
		get_admins(config)
	};
	let mut evil = vec!["evil@evil.com".to_string()];
	recipients.append(&mut evil);
	send(recipients)
}

#[dfpp::analyze]
fn blessed_and_unblessed_safe_sources(config: u8) {
	let mut recipients = if get_num() < 90 {
		get_staff(config)
	} else {
		get_admins(config)
	};
	recipients.append(&mut get_staff(config));
	send(recipients)
}

// The following fails only_send_to_allowed_sources because bless flows into recipients after the instantiation of safe_source_with_bless. This cannot be permitted because it looks identical in the graph as if we added staff and then did the check later in some irrelevant place. I think we do need to have some concept of the specialness of modifying fns like append, push, etc. 
#[dfpp::analyze]
fn conditional_modification(config: u8) {
	let mut recipients = empty_vec();
	let mut staff = get_staff(config);
	if get_num() < 90 {
		recipients.append(&mut staff);
	}
	send(recipients);
}

#[dfpp::label(safe_source, return)]
fn empty_vec() -> Vec<String> {
	vec![]
}

#[dfpp::label(bless_safe_source, return)]
fn get_num() -> u8 {
	10
}

#[dfpp::label(safe_source_with_bless, return)]
fn get_staff(config: u8) -> Vec<String> {
	vec![]
}

#[dfpp::label(safe_source, return)]
fn get_admins(config: u8) -> Vec<String> {
	vec![]
}

#[dfpp::label{ scopes, arguments = [0] }]
fn send(recipients: Vec<String>) {
}

fn main() {}
